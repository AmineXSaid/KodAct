# KodAct compared with OMP, DeepSeek Harness, Unsloth and Bionic

Second part of `docs/VERIFICATION_COMPARISON.md`, on the same mechanisms. Read
on 2026-11-10, against KodAct at `65365b7` (with the five changes of the first
report). Research only: nothing here is implemented.

| Name | What it is | What was read |
| --- | --- | --- |
| OMP | oh-my-pi (`omp`, installed from omp.sh), an open-source fork of Pi | `can1357/oh-my-pi` at `433a910`, source and `docs/` |
| DeepSeek Harness | `dsh`, open source | `deepseek-ai/deepseek-harness` at `fcc0ad4`, package READMEs and `packages/llm/llm/src/retry-policy.ts` |
| Unsloth | Not a harness: `unsloth start` launches another agent (Claude Code, Codex, `dsh`, OpenCode, Pi) against a local model | `unslothai/unsloth` at `322c4c1`, `unsloth_cli/commands/start.py` |
| Bionic | LM Studio Bionic, a closed-source desktop app | Search snippets only: `lmstudio.ai` was outside the network this was run from |

**V** means read in source or the project's own docs. **I** means inferred, or
not found after a search.

## Table

| Mechanism | KodAct | OMP | DeepSeek Harness | Unsloth | Bionic |
| --- | --- | --- | --- | --- | --- |
| **Check before finishing** | Reminder on unchecked changes, on by default. **V** | None in the harness; prompt only. An optional reviewer model (`advisor.enabled`) is off by default (`src/advisor/settings.ts`). **V** | None found. **I** | The launched agent's. | Unknown. **I** |
| **Verification state in the result** | `verification.status`: passed, failed, unchecked, read_back, no_changes. **V** | None found. **I** | None found. **I** | None. | Unknown. **I** |
| **Diagnostics after edits** | Parse check only (Python, JS, shell, Ruby, PHP, JSON), on by default. **V** | Language-server diagnostics after a write, on by default (`lsp.diagnosticsOnWrite: true`); after an edit, off (`lsp.diagnosticsOnEdit: false`); `lsp.formatOnWrite` off (`src/lsp/settings.ts`). **V** | Language server for navigation only: "Navigation is read-only" (`packages/lsp/lsp/README.md`). No diagnostics. **V** | The launched agent's. | Unknown. **I** |
| **Loop guard** | Reminder at 3; then a repeat of a file-tool call is refused until something changes. **V** | On by default (`model.toolCallLoopGuard.enabled`). Injects "NEVER call `{tool}` with those arguments again this turn" (`prompts/system/tool-call-loop-redirect.md`); threshold not read. Also a guard on reasoning or prose that loops while streaming (`model.loopGuard.enabled`). **V** | On in the base bundle: reminders at 3, 5 and 8 repeats. "The reminder is advisory: it never blocks" (`packages/guard/repeat-tool-reminder/README.md`). **V** | The launched agent's. | Unknown. **I** |
| **Empty reply** | "Act on what you have now", up to 3. **V** | "Stopped without actionable output; task incomplete. Continue…" with a retry count (`prompts/system/empty-stop-retry.md`). **V** | Retried as an API failure (`EMPTY_RESPONSE` is a retryable code). **V** | Text-form tool calls from small models are promoted back to structured calls, and one retry with a nudge when that fails; both on by default (`start.py`). **V** | Unknown. **I** |
| **Open todos** | `--autonomous` twice; `-p` once. **V** | `todo.reminders: true` by default (`src/tools/settings.ts`). **V** | Not found. **I** | The launched agent's. | Unknown. **I** |
| **API retry** | 10 tries, 5 s to 60 s; a spent quota is not retried; a long `retry-after` is stated or taken in full. Fallback model on overload or a missing model. **V** | 10 tries, 500 ms base, up to 5 min (`src/session/settings.ts`); model fallback and credential rotation, also on rate limits (`retry.fallbackOn`, `retry.rateLimitWalksChain`); `retry.waitForUsageReset` off by default. **V** | 5 tries, 500 ms to 10 s, for rate limit, server, timeout, transport and empty reply. An "always" mode retries quota and auth errors too, listed by its docs as a limitation. **V** | A longer stream timeout for slow local models; otherwise the launched agent's. **V** | Unknown. **I** |
| **Keep going until a goal is met** | `/goal`. **V** | `/loop` and a goal mode exist; not read in detail. **I** | `/goal`, automatic rounds capped at 256. **V** | The launched agent's. | Unknown. **I** |
| **Prompt tells the model to verify** | Lists the detected checks. **I** | "Verify proportionally: run the narrowest check that exercises the change… once." Sub-agents: "NEVER verify your changes… unless your assignment explicitly instructs it" (`prompts/system/system-prompt.md`). **V** | Not found. **I** | Sub-agent: "verify your work". Parent: "It is a small local model and can be wrong: ask it for the raw command output". **V** | Unknown. **I** |
| **Published measured effect** | None yet. | None found. | `BENCHMARK.md` says how to run benchmarks; no numbers. **V** | None. | None found. |

## Reading

- OMP is ahead on diagnostics: real language-server errors (types, imports)
  after a write, by default. KodAct's parse check sees syntax only.
- KodAct is alone on three things: the default-on finish reminder tied to
  which files changed, the verification state in the result, and refusing a
  repeated failing call (OMP and DeepSeek Harness only remind).
- Retries are comparable. OMP waits longer and can change model or credential
  on a rate limit. DeepSeek Harness's "always" mode retries a spent quota, the
  case KodAct now stops on.
- Third-party coverage of Bionic mentions automatic checkpoints for rolling
  changes back. That is undo, which KodAct has (`/rewind`), not verification.

## Proposals, ranked

Each follows `KODACT.md`. None is implemented or measured.

### 1. Language-server diagnostics after edits

The remaining "To build" of pillar 3, and what OMP does by default.

- **Real use:** an edit that breaks a type or an import, in this file or in a
  caller, is told with the edit's result. Today KodAct finds out only when a
  check runs, turns later, or not at all in a project with no detected check.
- **Change:** when the project's language server is installed, ask it for the
  diagnostics of the edited file after a write or an edit and attach the new
  errors. Start the server on first use; say nothing when it is missing, slow
  or reports nothing new.
- **Fix, don't fit:** the attached text is capped as a share of the model's
  window; a server that hasn't answered within a short deadline adds nothing,
  so a fast model is not held back. No language or project is named in the
  engine: the servers are a table, as the detected checks are.
- **Pillar / metric:** verification / false finish, turns per task.
- **Eval:** `rename-symbol`, `split-module`, and a new task where an edit
  breaks a caller in another file (the parse check cannot see it).

### 2. Use the fallback model on a rate limit that doesn't clear

- **Real use:** someone who set `--fallback-model` and whose first model is
  throttled for minutes keeps working on the second, where today KodAct waits
  on the first until its retries run out. OMP does this (`retry.fallbackOn`).
- **Change:** after a 429 has been waited out a few times in a row, or when
  the endpoint asks for a wait longer than a retry sleeps, switch to the next
  fallback model for the turn, with the notice KodAct already shows on an
  overload.
- **Fix, don't fit:** decided from the endpoint's answers, not from a model's
  name or size. With no fallback set, nothing changes.
- **Pillar / metric:** failure recovery / recovery rate, wall time.
- **Eval:** mock endpoint: the first model answers 429 with a long
  `retry-after`, the fallback answers; the run must pass on the fallback.

### 3. Remind again when a shell loop goes on

- **Real use:** KodAct reminds once per pattern. A shell, web or MCP call is
  never refused (it can work the fourth time), so a model that ignores the one
  reminder can repeat it for the rest of the turn. DeepSeek Harness reminds at
  3, 5 and 8.
- **Change:** for calls that are not refused, remind again at later counts,
  each time with the count and the unchanged result.
- **Measure first:** count the transcripts where a `repeated_failure` or
  `no_progress` event on a shell call is followed by the same call again.
- **Fix, don't fit:** counts of identical calls with identical results, never
  time; a model that changes its call never sees it.
- **Pillar / metric:** failure recovery / turns wasted after the first error.
- **Eval:** `build-recovery`, `switch-idea` (`loop_guards` and turns).

### 4. Cut a reply that repeats itself while it streams

- **Real use:** a model that loops in its reasoning or its prose runs to the
  output cap before KodAct reacts, which costs a full reply in time and
  tokens. OMP stops the stream when it sees the repetition.
- **Change:** watch the streamed text for the same block repeating; stop the
  request and send the existing "act on what you have now" reminder.
- **Fix, don't fit:** the trigger is repetition within one reply, as a share
  of that reply, with no fixed token count; a reply that does not repeat is
  never cut, whatever its length.
- **Pillar / metric:** failure recovery / wall time, cost, `act_reminders`.
- **Eval:** `encode-for-decoder`, and a mock endpoint that streams a
  repeating block.

### 5. Accept a tool call a small model wrote as text

- **To check first:** KodAct repairs invalid tool-call JSON. Whether a call
  written in the reply's text (a JSON object or a tagged block, with no
  structured call) is recognized was not checked for this report.
- **Real use:** local and small open-weight models do this often; the turn
  then ends on what looks like an answer. Unsloth promotes such text back to a
  call, and retries once with a nudge when it cannot.
- **Fix, don't fit:** applies only when the text parses as a call to a tool
  that exists with valid arguments; a model that calls tools properly is
  untouched.
- **Pillar / metric:** tool design / invalid-call rate, pass rate.
- **Eval:** a mock endpoint that answers with a text-form call.

### Not proposed

- **A reviewer model by default** (OMP's advisor): KodAct has `/advisor`, off
  by default as OMP's is. It doubles the model calls.
- **Retrying every error without end** (DeepSeek Harness's "always" mode): it
  retries a spent quota and a refused key, the cases KodAct now stops on.
- **Checkpoints** (Bionic): KodAct has `/rewind`.
