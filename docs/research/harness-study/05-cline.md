# Cline: what its source teaches KodAct

- **Source:** `cline/cline` at commit `72b3d71` (2026-11-13).
- **Licence:** Apache-2.0 (`LICENSE` checked). Adaptation with attribution
  is allowed. Nothing is copied here.
- **Layout note:** the repository is now an SDK (`sdk/packages/{shared,core,
  agents,llms}`) with apps on top (`apps/cli`, `apps/vscode`).
  `docs/REFERENCES.md` read only the README in October.
- **Read in full:** `sdk/packages/shared/src/prompt/cline.ts`,
  `prompt/system/act.ts`, `prompt/system/yolo.ts`.
- **Read in part:** the tool definitions
  (`core/src/extensions/tools/definitions.ts`), the compaction modules
  (`core/src/extensions/context/basic-compaction.ts`,
  `compaction-shared.ts`), the limits at the top of
  `agents/src/agent-runtime.ts`.
- **Not read:** the VS Code extension, the hub and cloud services, the
  team and multi-agent tools, checkpoints, MCP, plugins, the evals.

## 1. How the prompt is built

Two short base prompts, chosen by mode, with placeholders for the
platform, date, IDE, working directory, rules and workspace metadata:

- **Act** (3,700 characters): gather context first, follow conventions,
  no placeholders in code, show a plan with the first tool calls, batch
  every independent call into one response, verify edited files at the
  end, and "a response without tool calls is the final answer".
- **Yolo** (4,600 characters), for unattended runs: the same, plus a
  block on verification and a way to end the run.

Plan mode is not a third prompt. It is a rule block appended to the Act
prompt, and each user message is wrapped in a tag that carries the mode it
was sent in. The prompt explains: the newest message's mode governs, and a
`<mode_notice>` marks the switch. So the system prompt does not change
when the person toggles the mode (the cache survives), and the model can
tell which earlier messages were written under plan rules.

### What the unattended prompt says

It is the closest relative of KodAct's `05-autonomous.md`, and the two
agree on most points. Cline adds:

- "Be proactive and avoid overthinking": once the next action is clear, do
  it; do not revisit a settled decision without new evidence.
- After repeated failed fixes without new evidence, stop making similar
  edits and test the assumption with a minimal reproduction.
- Put full code and commands in the tool arguments; do not draft them in
  the plan first. (KodAct says the same about reasoning.)
- "Verify by execution, never by assumption", with a tell: catching
  oneself thinking "this should work" means the check has not been run.
- With no tests given, build the check: run the program, confirm each
  required output file exists at the exact path, read it back.

## 2. Mechanisms worth knowing

### 2.1 Ending a run is a tool call with a claim attached

In unattended mode the run ends only when the model calls
`submit_and_exit(summary, verified)`. A reply without it does not end the
run. `verified` must be true only if the model's own tool output shows the
requirements are met. The description repeats the checklist: re-read the
task, confirm each file exists where asked, run the tests or the solution.

Two effects. The model cannot end a run by accident with a sentence that
announces a next step (KodAct catches that case after the fact and sends
the turn back). And the harness receives a structured claim it can compare
with what it observed: a run that says `verified: true` when no check ran
after the last edit is a false finish caught at the source.

KodAct already computes `verification.status` from what it observed. A
finish tool would add the model's claim next to it. It applies only to
unattended runs; a person at a terminal ends a turn by reading the reply.

### 2.2 Tools that take arrays

`read_files` takes a list of paths, each with an optional line range.
`run_commands` takes a list of commands that are independent and safe to
run together. One call, one result per item. For models and endpoints that
do not emit parallel tool calls reliably (many open-weight models behind
gateways emit one call per reply), this gets the same saving in turns
without relying on parallel calls.

KodAct's Read takes one file. Parallel calls work where the model and
endpoint support them; where they do not, reading four files is four model
calls.

### 2.3 Compaction without a model call

`basic-compaction.ts` folds history with no summarizer:

- the person's typed messages always survive;
- the current turn keeps its newest messages within the target (70% of the
  budget), cut only at a point that does not split a tool call from its
  result;
- older turns keep their final answer, without thinking blocks, newest
  first, while they fit;
- everything dropped is replaced by a **mechanical record** attached to
  the surviving prompt: files read (with line ranges), files edited (with
  the line ranges taken from the diffs), commands run (first 100
  characters each), and the last three assistant texts word for word;
- what survived one compaction is frozen and never folded again.

It is instant, free, cannot hallucinate, and cannot be steered by content
in the history. It loses the reasoning ("why") a model summary keeps. A
second strategy with a summarizer exists beside it; that one asks for
thinking to be off and caps the summary's output.

KodAct's summary is written by the model. A mechanical record of files
read, files edited and commands run could be **attached to the model's
summary at no cost**: those are exactly the facts a summary gets wrong or
drops, and KodAct already has them in the transcript.

### 2.4 The trigger scales with the window

Compaction starts at 90% of the usable input (itself 90% of the window
when the model reports no input limit), targets 70%, and preserves about
20,000 recent tokens. The estimate is corrected by the provider's real
token count, with a cap on the correction so one odd reading cannot make
it compact every turn.

### 2.5 Plan mode with a hard backstop

The shell stays available in plan mode for inspection. The prompt says
so, and a command guard (`command-guard.ts`) rejects file-changing
commands (`rm`, `mv`, `sed -i`, redirection outside the temp directory,
git commands that change the tree, installs) with a tool error before
approval. The prompt tells the model the guard exists, so it does not try.

### 2.6 Smaller points

- Truncated replies are recovered at most three times, provider errors
  retried three times with a 15-second cap; then the run fails instead of
  looping.
- Command output over the limit keeps start and end, and the description
  says so and suggests filtering.
- The read tool reports a long file's total line count so the model can
  page.
- Remote URLs have credentials stripped before they reach the prompt.
- The shell tool's description is rebuilt per request, so it names the
  shell in use even if it changed mid-session.

## 3. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| L1 | Read accepts several files (or ranges) in one call | On a model or gateway that emits one tool call per reply, reading four files costs one model call, not four | Tool design / turns per task | A model that already calls in parallel is unaffected | A schema field; no extra text if the description stays the same length |
| L2 | Attach a mechanical record (files read with ranges, files edited, commands run) to every compaction summary | After compaction the model knows what it already read and changed, even if the summary omitted it | Context management / pass rate after compaction, repeated reads | Compaction is rare; when it happens the record is small | None before compaction; no model call |
| L3 | Unattended runs end with a finish tool that carries `verified` and a summary; the result records the claim next to `verification.status` | A script can tell "the model says verified and KodAct saw a passing check" from "the model says verified and no check ran" | Verification / false-finish rate | Model-neutral | One tool definition in `--autonomous` runs only; not in interactive sessions |
| L4 | Mode changes travel on the user message, not in the system prompt | Toggling plan mode does not rewrite the cached prefix | Context management / cost per task | Same | None |
| L5 | Plan mode rejects file-changing shell commands with a tool error and says so in the prompt | A plan cannot change the tree through the shell | Compatibility, verification / changes made in plan mode | Same | None |
| L6 | Prompt line for unattended runs: after repeated failed fixes with no new evidence, test the assumption with a minimal reproduction | Fewer loops of near-identical edits | Failure recovery / turns after first error | Model-neutral | About 30 tokens, unattended runs only |

On L5: KodAct's plan mode already denies every tool that is not read-only
(`crates/kodact-permissions/src/lib.rs`, "plan mode allows only read-only
tools"). What is left to check is whether a read-only shell command
(`git log`, `ls`) is allowed there, as it is in Cline, or whether the
model must plan without the shell.
