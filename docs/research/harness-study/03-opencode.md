# opencode: what its source teaches KodAct

- **Source:** `sst/opencode` at commit `fb11429` (2026-11-13).
- **Licence:** MIT (`LICENSE` checked, copyright 2025 opencode). Adaptation
  is allowed with the copyright notice kept. Nothing is copied here.
- **Layout note:** the code moved since `docs/REFERENCES.md` was written
  (2026-10-06, from the docs site). The harness now lives in
  `packages/core/src`; `packages/opencode` no longer exists.
- **Read in full:** every prompt text file in `packages/core/src`:
  `session/runner/prompt/system.txt`, the per-family prompts in
  `plugin/system-prompt/` (`anthropic`, `gpt`, `gpt-astra`, `trinity`), the
  goal prompts in `plugin/goal/`, `tool/patch.txt`, `tool/todowrite.txt`,
  `session/runner/max-steps.ts`.
- **Read in part:** `session/compaction.ts` (constants, template, the
  background-summary and shrinking logic), `tool/plugin/edit.ts`,
  `tool-output.ts`, `plugin/tool-input-repair.ts`, `plugin/warming.ts`, the
  descriptions in `tool/plugin/{shell,read,subagent}.ts`.
- **Not read:** `kimi.txt` and `meta.txt` prompts, the provider plugins,
  the TUI, desktop app, server, code mode, permissions and LSP packages.

## 1. How the prompt is built

The base system prompt is **756 characters**: a role line, three harness
facts (Markdown is rendered, `<system-reminder>` blocks come from the
harness, run independent calls in parallel), two lines on communication, two
on working in a codebase. Tool guidance is injected at a placeholder, so it
exists only for tools that are present.

Model families get additions:

- For one family, a single paragraph on comment density (match the
  surrounding code; where it has none, add none).
- For another, sections on intermediate commentary, steering, destructive
  actions and autonomy ("do not infer authorization for work beyond the
  request").
- An older long prompt (`trinity.txt`, 7,700 characters) remains for models
  that need it: strict four-line answers with examples, "do not add any
  comments".

Two things stand out. First, the newest prompts are the shortest: the team
removed text as models improved, and what remains is facts about the harness
and rules about other people's work. Second, both newer prompts say **not
to spawn sub-agents unless the person asks**, the opposite of Gemini CLI's
"you must delegate". Delegation costs a cold start; opencode treats it as
the expensive path.

KodAct's prompt is 6,200 characters before the unattended section. By
opencode's standard, several of its lines are advice a model could give
itself (rule 8 of `docs/goal.md` says the same). This is the strongest case
in the study for an A/B of a shorter prompt.

## 2. Mechanisms worth knowing

### 2.1 Compaction designed around the prompt cache

This is the most developed compaction in the six repositories.

- **Trigger:** 15% of the window is kept free (at least 16,000 tokens).
  Both are shares or floors, not sizes fitted to one window.
- **The summary request reuses the cache.** The conversation is sent
  unchanged with the summary instruction appended, so the provider reads the
  whole prefix from cache and the summary costs little.
- **Background summary.** About 60,000 tokens before the limit, a summary of
  everything up to the last response is started in the background, while
  the cache is still warm. It is held, not applied: applying it early would
  rewrite the prefix and lose the cache for every step until compaction is
  really due. When compaction is due, the held summary is swapped in with
  the newest messages kept beside it word for word. The person does not
  wait 30 to 60 seconds for a summary at the limit.
- **If the summary request is too long**, the history is flattened to a
  text transcript and shortened by level, oldest first: tool outputs cut to
  2,000 characters, then outputs removed and inputs cut, then only the
  user's text, then the exchange omitted. Every exchange reaches one level
  before any reaches the next. After a "too long" refusal the target
  shrinks by steps (85%, 70%, 50%, 35%).
- **An update prompt.** A second compaction does not summarize a summary
  from scratch: it is told to update the existing checkpoint, that newer
  history wins, and to move finished work out of "active".
- **The template** has seven sections: objective, requirements, decisions,
  work state (completed, active, blocked), next move, relevant files (at
  most 15, with the reason each matters), important context. Its rules:
  leave out what the next agent can recover from the code; keep exact
  paths, commands and error strings; carry forward only the requests still
  open, word for word; record whether changes are committed, pushed or
  merged; leave out repository instructions, which are re-attached anyway.
- **A nudge:** a reply that does not fill the template gets one reminder
  before it counts as a failure.

KodAct summarizes at the limit, in the foreground. On a small model behind
a gateway a summary takes a long time, and that is exactly where
compaction happens most.

### 2.2 Edit: three passes and two kinds of hint

`edit.ts` matches the exact text, then with typographic quotes, dashes and
spaces normalized, then ignoring trailing whitespace per line. The tool
description tells the model this is what happens. Matching is done on the
view with LF endings that Read shows, and the file's own endings are kept.

When nothing matches, the error carries one of two hints:

1. "newString is already in the file, so this edit may already be applied."
   A model that retries an edit after a lost result gets the fact that ends
   the loop.
2. The closest lines, found by voting: each line of the search text that
   matches a file line ignoring whitespace votes for a starting position;
   at least half must agree. Only the span that matched is shown, numbered
   like Read output.

The hint is given only when reading that file is already permitted, so an
edit error cannot leak a file the model may not read.

KodAct already shows the closest window. It does not have the
"already applied" hint or the typographic pass.

### 2.3 Tool input repaired from the schema

`tool-input-repair.ts` fixes a call only where the tool's schema makes the
fix unambiguous: an object sent as a JSON string, a number or boolean sent
as a string, a `null` for an optional field, an extra field on a closed
object, a string where an array of one is expected. It also maps a tool
name a model carries over from another harness (`bash` for `shell`) when no
tool has that name, runs the right tool, and adds a note naming the tool to
call next time.

KodAct repairs invalid JSON and calls written as text. It does not coerce
types against the schema: a small model that sends `"limit": "20"` gets a
validation error and spends a turn.

### 2.4 The last step has no tools

When the step limit is reached, one more model call is made with tools
disabled and a prompt that asks for what was done, what remains and what to
do next. The run ends with a usable hand-off instead of stopping mid-task.
KodAct reminds the model when three calls are left; if the model ignores
it, the run ends as `error_max_turns` with whatever the last message was.

### 2.5 Cache warming

An opt-in plugin sends a keep-alive request a little before the provider's
cache would expire (every 4 minutes by default, for up to 30 minutes), so a
person who reads the result and replies five minutes later still gets the
cached price and speed. It never runs when the cache already outlives the
window.

### 2.6 Tool output

Any tool result over 2,000 lines or 50 KiB is cut, saved to a file kept
seven days, and the result names the file. KodAct does the same with
different numbers.

### 2.7 Goals judged by a separate call

The goal plugin ends each turn with a judge prompt: stop acting as the
assistant, decide from the evidence in the conversation only, the
assistant's claim is not evidence, answer one line of JSON (`done`,
`continue` with the next step, `blocked`, `paused`). The kickoff prompt
tells the worker in advance that a reviewer trusts only evidence it can
see, so the worker shows command output. The objective is wrapped as
untrusted data everywhere it appears.

### 2.8 Shell and sub-agents

- The shell tool's background flag says the model will be notified on
  completion and must not poll. A `workdir` parameter replaces `cd`.
- A sub-agent call takes a `sessionID` to send a follow-up to an existing
  sub-agent, running or finished, so its context is reused instead of
  starting cold.

## 3. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| O1 | Coerce tool input to the schema where unambiguous (string to number or boolean, stringified object or array, `null` for an optional field) | A small model's malformed call runs instead of costing a turn | Tool design / invalid-call rate | Never triggers on valid input | None |
| O2 | Edit's "not found" error says when `new_string` is already in the file | A retried edit stops after one error instead of looping | Failure recovery / turns after first error | Same | None: only on a miss |
| O3 | Start the summary in the background before the limit and hold it until compaction is due | No long pause at the limit; the summary is read from cache, so it costs less | Context management / wall time and cost per compaction | The lead is fixed in tokens by how fast a conversation grows, so on a 1M window it starts at the same distance from the limit, just later | None before the lead; one background call that would have happened anyway |
| O4 | A second compaction updates the previous summary under an "update" instruction | Long sessions do not lose early requirements to a summary of a summary | Context management / pass rate on `deliverable-after-compaction` with two compactions | Rare there | None |
| O5 | When the turn limit is reached, one final call without tools asks for done / remaining / next | A `--max-turns` run ends with a hand-off a person or script can use | Long-horizon / usable results at the limit | Same | One call, only at the limit |
| O6 | Send a follow-up to an existing Task agent | Review-then-fix with one sub-agent reuses its context | Context management / tokens per task | Same | A field on the Task tool |
| O7 | A/B a short prompt: harness facts and other-people's-work rules only | Lower cost on every request; less for a strong model to wade through | Every pillar / quiet-check tokens and pass rate | This is the case it is for; a small model may need the longer text, so the A/B runs on both | Saves tokens |
| O8 | Unknown tool name that maps to a KodAct tool (`shell`, `run_shell_command`, `apply_patch`…) runs as that tool with a note | Models trained on other harnesses do not burn a turn on the name | Compatibility / invalid-call rate | Same | None |
| O9 | Opt-in cache keep-alive while a person is reading | Replies after a pause stay fast and cheap on providers that cache | Cost per task | Same | Off by default; small paid requests when on |

Compaction details worth copying into KodAct's summary instruction without
any new machinery: cap "relevant files" and ask for the reason each
matters; "leave out what can be recovered from the code"; record whether
work is committed or pushed.
