# KodAct goals

## Objective

The goal is the best terminal coding agent per model. Success means the
highest measured task success, at the lowest cost and wall time, with any
given model.

The reference CLI (v2.1.290) is the **compatibility baseline**: its flags,
stream-json protocol, settings, hooks and tools are matched so that KodAct
can replace it. It is not the ceiling. A better harness can beat a weaker one
running the same model, so the harness gets designed, measured and improved as
the product.

## Rule: measure, don't assume

A change that claims to improve the harness must show it on the evaluation
suite (`kodact-eval`). It is compared A/B against the previous build, using the
same model, the same tasks and repeated runs.

| Metric | Definition |
| --- | --- |
| Pass rate | Share of tasks whose checker passes (pass@1, averaged over N runs) |
| Cost | USD per task, and per *passed* task |
| Turns | Model calls per task |
| Wall time | Seconds per task |
| Recovery rate | Share of runs that hit a tool or API failure and still pass |
| False finish | Share of runs that report success while their checker fails |

## Rule: fix, don't fit (read before every fix)

KodAct runs on whatever model the person chooses, from a small model behind
a company gateway (a 100k window, a 32k reply cap) to a large one with a
window of 200k to 1M tokens. A problem found on one model, run or benchmark
task is fixed in the harness for every model, never tuned to the case that
showed it: limits come from the model, a fix says how it behaves on a large
model and must help there or stay out of the way, it fixes a mechanism and
not a task, and the tests cover a small and a large window. The full rule,
and how it is checked without a large model at hand, is in `docs/goal.md`.

## The five pillars

Each pillar lists what KodAct does about it and what shows that it works.

### 1. Context management

- **Done** (contract C15):
  - compaction in tiers: sticky micro-compaction of stale tool results,
    summarization near the limit, and compact-and-retry when a prompt is too
    long;
  - sub-agents, so exploration runs in a separate context and only its result
    comes back;
  - budgets on tool output: long Bash output (30k characters), and any other
    tool's output over 50k, keeps its head and tail. The full text is saved
    under the cache directory, which can be read back without a prompt, and
    the result names the file;
  - no repeated reads: reading the same range of unchanged content returns a
    short note. That stops once the earlier result was cleared by compaction;
  - memory files and the task list re-attached after compaction;
  - the person's requests carried word for word through every compaction and
    a resume, within a tenth of the model's window, whatever the summary kept
    (task `deliverable-after-compaction`);
  - each model's own window: `modelLimits`, then what its endpoint lists, then
    the built-in table. `KODACT_CONTEXT_WINDOW`, one value for every model, only
    fills in for a model with none of these, so a value set for a small model
    never shrinks a large one (`evals/context-window/`, in every mode);
  - a window the endpoint states anywhere is learned: its model list,
    LiteLLM's `/model/info`, or a refusal for length ("maximum context length
    is N tokens"), after which the request goes again sized to fit, instead of
    compacting on a guessed window.
- **Measured by:** pass rate and cost on long tasks, and context tokens per
  turn (task `log-needle`).

### 2. Failure recovery

- **Done** (contract C13):
  - API retries with backoff, fallback models, and interrupt with rewind.
  - Edit's "not found" error gives the next step:
    - text that matches except for whitespace, quoted with line numbers;
    - Read's line-number prefixes copied into `old_string`;
    - otherwise the closest window, scored.
    Ambiguous matches name their lines.
  - Stuck-loop detection: the same failing call three times, the same result
    three times, edits that undo each other, or five failures in a row each
    trigger one reminder to change approach.
  - Truncated output:
    - a tool call cut off by `max_tokens` is answered with an error, not
      sent as a failed turn;
    - cut-off text is continued, up to 3 times;
    - the output cap is raised to 64k for the rest of the turn;
    - no request asks for more reply than the model's window has left after
      its prompt, so endpoints that count the reply against the window don't
      refuse it; on a large window the cap goes out unchanged.
- **To build:** recovery after a crash (resume an interrupted turn from the
  transcript).
- **Measured by:** recovery rate, and turns wasted after the first error
  (tasks `build-recovery`, `makefile-tabs`).

### 3. Verification

- **Done** (contract C12):
  - check commands detected from manifests (Cargo, package.json scripts, Go,
    Python, Make, scripts, Maven/Gradle and more) or set in
    `verification.commands`, and listed in the environment prompt;
  - a built-in check before a turn ends: with changes unchecked since the
    last change, the model gets a reminder (once by default) naming the files
    and the checks;
  - the reminder asks the final message to state what was verified and what
    was not;
  - one reminder per set of changes, and an answer or data file read back
    after its last change counts as checked (task `answer-file`).
  - `"reminders": false` or `KODACT_REMINDERS=0` turns all of KodAct's
    reminders off (this one, the loop guard, the act, attempt, next-step and
    todo reminders, the budget and time wrap-up), for A/B runs against the
    model alone.
  - It is on by default; `KODACT_VERIFY=0` turns this one off, and
    `kodact-eval` records `verify_reminders` per run.
- **To build:** LSP diagnostics and formatter feedback after edits (the
  OpenCode pattern).
- **Measured by:** false-finish rate and pass rate (tasks `fix-median`,
  `slugify-trap`, `invoice-rounding`).

### 4. Tool design

- **Done:** precise errors, read-before-write, CRLF-safe Edit, parallel
  read-only batches, results returned in call order, and repair of invalid
  tool-call JSON (open-weight models' most common invalid call).
- **To build:**
  - tool descriptions tuned against the eval suite;
  - structured results everywhere;
  - output that helps the model's next step: counts, truncation markers, and
    what to try when a call fails.
- **Measured by:** turns per task and invalid-call rate.

### 5. Long-horizon execution

- **Done:**
  - the task list (TodoWrite) survives compaction: it is re-attached to the
    summary and recorded as `system/todos`. It also survives resume: it is
    rebuilt from the last TodoWrite call or that record;
  - budget awareness: one reminder to wrap up when 3 model calls (of at least
    6) or 15% of the budget are left;
  - unattended runs (`--autonomous`, contract C21): no question tools, and a
    reminder to attempt before giving up. In the first Terminal-Bench subset,
    3 of 4 failures ended after 3 turns or fewer, without a real attempt;
  - unattended finish checks (C21): a reply that announces a next step without
    a tool call, or ends with open todos, is sent back once or twice. In the
    quick-thinking run of 2026-10-09, gpt2-codegolf ended "success" on "Let me
    examine the context around those markers." with no file written;
  - unattended API retries (C21): a failed model call waits and tries again
    while the run has time. In the deep-thinking run of 2026-10-09, 7 of 10
    trials ended after 0 turns on HTTP 429, and 6 runs across the jobs ended on
    a connection lost mid-reply;
  - time awareness (`--max-time`): the limit is stated up front, one
    reminder comes when a fifth of the time is left, and the run stops at the
    limit with an error result (`stop_reason: "max_time"`), a call in flight
    included. Benchmark tasks have hard time limits; a run cut off mid-change
    scores nothing;
  - resumable sessions, and unattended headless runs.
- **To build:**
  - a notes file;
  - checkpoints the model can roll back to itself.
- **Measured by:** pass rate on multi-step tasks, and success after a resume.

## Priority order

1. `kodact-eval`: the task suite, the runner and A/B reports. **Done.** There
   are 13 validated tasks across the pillars, plus `validate`, `run` and
   `compare`. No measured baseline exists yet: the first `kodact-eval run` with
   a real API key sets it. How to record it, what to commit and how to read
   the A/B report: docs/BASELINE.md, "Recording the first real baseline"
   (its commands are checked against `kodact-eval` by a test).
2. Sub-agents (the Task tool, built-in Explore and Plan agents), to isolate
   context.
3. The verification loop. **Done**, without LSP feedback.
4. Recovery: Edit match suggestions, stuck-loop detection, `max_tokens`
   recovery. **Done.**
5. Budgets on tool output, and no repeated reads. **Done.**
6. A plan that survives compaction. **Done** (the task list; a notes file is
   still to build).
7. Parity features in order of use:
   - **Done:** MCP (stdio, HTTP and SSE client, `kodact mcp`, `kodact mcp
     serve`), web tools, plan mode and AskUserQuestion, skills, output styles.
   - **Mostly done:** slash commands, with one registry, goals (C18) and
     scheduled prompts (C19), and background subtasks (C20).
   - **Mostly done:** the terminal UI (docs/TUI.md): an inline viewport over
     the terminal's scrollback, dialogs, pickers, Ctrl+R, `@` completion
     and `@file` attachments in every mode, immediate commands mid-turn
     (TUI and stream-json),
     `/theme`, `/copy`, `/keybindings`, `/statusline`, `/terminal-setup`.
     Checked against mocks and in a pseudo-terminal; the real-terminal checks
     are in docs/CHECKLIST.md.
   - **Checks:** the real-API and real-terminal checks are one ordered run of
     55 (docs/CHECKLIST.md: print mode, interactive, special setups), set up
     by `scripts/check-setup.sh`; a test keeps every command in it valid.
     They haven't been run with a real key yet.
   - **Done since:** the editor-style screens (`/diff` viewer, `/context`
     grid, `/hooks` editor, `/agents` wizard), key rebinding
     (`keybindings.json`), `/color` and `/focus`. `/tui` and `/scroll-speed`
     are out (PARITY).
   - **Still to build:** nothing from the TUI list; see PARITY for smaller
     gaps. Earlier list, kept for reference: key rebinding, `/color`, `/focus`, `/tui`, `/scroll-speed`, and the
     editor-style dialogs (`/hooks`, `/agents` wizard, `/diff` viewer,
     `/context` grid).
8. Remaining flags and polish.
