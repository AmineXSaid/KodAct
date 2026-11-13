# Codex CLI: what its source teaches KodAct

- **Source:** `openai/codex` at commit `16280e1` (2026-11-13), read from a clone.
- **Licence:** Apache-2.0 (`LICENSE` checked; `NOTICE` credits Ratatui and others).
  Text may be adapted with attribution. Nothing is copied here: short phrases
  are quoted and marked, everything else is described.
- **Read in full:** the base instructions
  (`codex-rs/protocol/src/prompts/base_instructions/default.md`), the
  per-model prompt `core/gpt-5.2-codex_prompt.md`, the newest model's
  instruction template in `models-manager/models.json`, and every template
  under `prompts/templates/`, `collaboration-mode-templates/`,
  `ext/goal/templates/`, `core/templates/`.
- **Read in part:** `core/src/compact.rs`, `utils/output-truncation`,
  `apply-patch/src/seek_sequence.rs`, the tool specs in
  `core/src/tools/handlers/` (shell, apply_patch, context tools), the
  constants of `core/src/unified_exec/`, `core/src/context_manager/normalize.rs`,
  `shell-command/src/command_safety/`, `memories/README.md`.
- **Not read:** the TUI, app server, cloud tasks, sandbox implementations,
  realtime and plugin crates (about 150,000 lines in `core` alone; the parts
  above are the ones that shape what the model sees).

## 1. How the prompt is built

Codex does not have one prompt. `models.json` carries, per model, an
`instructions_template` plus separate messages for approvals, collaboration
modes, sub-agent roles, context-budget reminders and auto-review. The
harness assembles them, and what changed between turns is sent as an update
("world state": environment, permissions, tools, AGENTS.md, mode), not by
rewriting the system prompt. Two consequences:

1. The system prompt stays byte-identical for the whole session, so the
   provider's prompt cache keeps hitting.
2. A mode change (plan to default, sandbox change) is one short message that
   says the previous mode's rules no longer apply.

The same table sets, per model: output truncation (10,000 tokens for every
current model), whether calls run in parallel, the edit tool's form (a
grammar-constrained free-form tool or JSON), the default reasoning effort and
verbosity, and the share of the window that counts as usable (95%).

### How the prompt changed across model generations

The older default prompt (about 21,000 characters) spends most of its length
on message formatting: preambles with eight examples, plan quality with six
examples, header and bullet rules. The prompt for the codex-tuned model drops
to 7,500 characters and keeps only what the model cannot infer: dirty-worktree
rules, the review format, when to skip the plan tool. The newest template
(21,000 characters again) spends its length on different subjects:

| Subject | What it says, in short |
| --- | --- |
| Permission | Ask like a colleague would. Authorization persists across turns. Do all the reviewable work first so approval is the last step. When stopping to ask, say which rule or file required it. |
| Persistence | A request phrased as "can you…" is a request to do it. Do not stop at a plan or a partial result. |
| Steering | A message sent mid-work steers the task; it replaces the task only when it clearly cancels it. |
| Compaction | The model is told that compaction happens, that all user requests survive it, and that it must not restart or repeat updates after one. |
| Writing | Prose over lists, no headings, a list of banned filler phrases, lead with the outcome. |
| Testing | Run the checks that fit the change; once they pass, repeat only after new changes. |

The lesson is the direction of travel: less formatting instruction, more on
authority, persistence and what happens to the conversation itself.

## 2. Mechanisms worth knowing

### 2.1 Compaction keeps the user's words and drops the rest

`compact.rs` asks for a short hand-off summary (the prompt is five lines),
then rebuilds history as: the fresh environment context, the user's messages
word for word (newest first, within 20,000 tokens; the one that crosses the
limit is cut), and the summary under a prefix that tells the next model
another model wrote it. Tool calls and results are all dropped. If the
summary request itself is too long, the oldest item is removed and it tries
again, which keeps the cached prefix.

KodAct already carries requests word for word (within a tenth of the window,
which scales where Codex's 20,000 does not). What Codex adds is the retry:
**drop the oldest item when the summary call overflows**.

### 2.2 A context window the model manages itself

For the newest models a second scheme exists (`token_budget` in
`models.json`, off by default): a `notes` tool, a `history` tool that reads
earlier windows by item id, `get_context_remaining`, and `new_context`. Near
the limit the model is told how many tokens remain and to save a checkpoint,
then starts a fresh window that contains only its notes. Every non-assistant
item carries an id so notes can point back at it.

KodAct has the Notes tool (opt-in, rarely called in measured runs). The
Codex design explains why it may go unused: in KodAct nothing tells the
model the window is nearly spent, so it has no moment to write notes. Codex
gives it the number.

### 2.3 Commands that yield instead of timing out

`exec_command` takes `yield_time_ms` (default 10 s, range 250 ms to 30 s).
A command still running then is not killed and not an error: the call
returns the output so far with a session id, and `write_stdin` polls it or
sends input (an empty write is a poll). The process runs in a PTY, so
prompts, REPLs and password questions can be answered. The model also sets
`max_output_tokens` per call. Up to 64 such sessions live at once.

KodAct continues a timed-out command in the background and has BashOutput
and KillShell. It cannot **send input to a running command**. A program
that asks a question (an installer, `git rebase` without `-i` guards, a
REPL) blocks until the timeout.

### 2.4 Patch matching that forgives typography

`seek_sequence.rs` looks for the patch's context lines four times, each
looser: exact, ignoring trailing whitespace, ignoring both ends, then with
typographic dashes, curly quotes and non-breaking spaces mapped to ASCII.
Models often emit `'` where the file has `’`, and the reverse.

KodAct's Edit suggests the whitespace-only near match in its error but does
not apply it, and has no typographic pass (`edit.rs` has no such mapping).

### 2.5 Output truncation states what was lost

Truncated output starts with a warning that gives the original token count
and the total line count, then head and tail. The model can decide whether
the middle matters. KodAct keeps head and tail and names the saved file,
which is better for recovery; the line count is a cheap addition.

### 2.6 History is repaired before every request

`normalize.rs` ensures each tool call has an output (a synthetic one is
inserted for calls a fork or crash left unanswered) and each output has a
call. KodAct's resume does the equivalent for a killed turn.

### 2.7 Approvals reviewed by a second model

With `approvals_reviewer = auto_review`, a request to leave the sandbox goes
to a classifier prompt that rates authorization (did the user ask for this)
and risk (exfiltration, credential probing, persistent weakening,
destruction) separately, and blocks on low authorization or high risk. A
rejection comes back with the instruction not to work around it and to
finish the unaffected work. The policy text is the most careful statement
of "what is a destructive action" in any of the six repositories: for
example, a reset of named files is a local edit, a reset of a branch is
not; version control is not assumed.

KodAct has a pattern scanner. A model reviewer is a larger step and costs a
model call per escalation; it is listed under ideas, not recommendations.

### 2.8 Goals with a completion audit

The goal continuation prompt (`ext/goal/templates/goals/continuation.md`)
is worth reading for KodAct's `/goal`. Its points:

- Each turn is classed as progress, a verified wait, or no progress; a
  restated status is no progress.
- Completion is "unproven" until every requirement of the objective has
  named evidence, read from the current state. A narrow check does not
  support a broad claim.
- "Blocked" may be declared only after the same blocker for three turns in
  a row, never because the work is hard.
- The objective is wrapped as untrusted data, not instructions.

### 2.9 Plan mode that separates two kinds of unknowns

Plan mode's prompt divides open questions into facts that can be found in
the repository (find them, never ask) and preferences (ask early, with two
to four options and a recommended default, and proceed on the default if
unanswered). The plan must leave no decision to the implementer.

### 2.10 Review rubric

`/review` uses a fixed rubric: flag only what the author would fix, that
the change introduced, with provable effect on other code; one paragraph a
finding; priority tags; a JSON result with a confidence score and an
overall verdict.

## 3. Candidates for KodAct

Each is stated the way `KODACT.md` asks. None is implemented; each needs its
eval task and an A/B before it is on by default.

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| C1 | Edit applies a match that differs only by typographic quotes, dashes or non-breaking spaces (and says so in the result) | An edit to a Markdown or localized file with curly quotes succeeds the first time instead of failing and costing a re-read | Tool design / turns per task, invalid-call rate | Same behaviour; a strong model hits this less often, so it stays out of the way | None: runs only after the exact match fails |
| C2 | Send input to a running command (a `BashInput` tool or a `stdin` field on BashOutput) | A command that stops at a prompt can be answered instead of waiting out the timeout | Tool design / turns, wall time | Helps equally | One more tool definition (about 300 characters) |
| C3 | Tell the model the window is nearly spent, once, with the number left, before compaction, when Notes is on | Long sessions: the model writes its notes at the moment they matter, instead of never | Context management / pass rate after compaction | Threshold is a share of the window, so on 1M it fires late or never | One message, only near the limit |
| C4 | Truncated output states the total line count | The model knows whether 40 or 40,000 lines were cut before deciding to read the saved file | Context management / context tokens per turn | Same | About 10 tokens, only when truncating |
| C5 | When the summary call is refused for length, drop the oldest items and retry | A session that reached the limit on a small window still compacts instead of ending in an error | Failure recovery / recovery rate | Never triggers | None |
| C6 | Tell the model in the prompt that compaction exists and that a message sent mid-work steers the task | After compaction the model continues instead of restarting or re-announcing; a correction mid-task is not treated as a new task | Long-horizon / pass rate after compaction | Two sentences, true for any model | About 60 tokens per request |
| C7 | `/goal`: classify each turn (progress, verified wait, none) and require named evidence per requirement before "done" | Unattended goals stop claiming completion on partial work | Verification / false-finish rate | Wording is model-neutral | Only in goal runs |
| C8 | Plan mode: split unknowns into discoverable facts and preferences | Fewer questions the repository could have answered | Compatibility, verification / turns | Same | Only in plan mode |

Ideas noted but not proposed now: a model reviewer for escalations (2.7), a
`/review` rubric (2.10), per-model prompt tables (section 1; this conflicts
with "prompts speak to every model" in `docs/goal.md` unless the table is
keyed on measured behaviour, not on model names).
