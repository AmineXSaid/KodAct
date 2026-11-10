# How four CLIs verify the agent's work, compared with KodAct

Research only: no code was changed for this report. Read on 2026-11-10.

| Project | What was read | Revision |
| --- | --- | --- |
| KodAct | `crates/kodact-engine/src/{engine,verify,stuck}.rs`, `docs/GOALS.md` | `9560698` |
| AlphaCode | `github.com/dragonked2/alphacode`, source | `8317831` |
| Claude Code | `code.claude.com/docs` pages, `CHANGELOG.md` of `anthropics/claude-code` (`ce6cb8e`), strings of the 2.1.322 binary | 2.1.322 |
| Pi | `github.com/badlogic/pi-mono`, source and docs | `75d5963` |
| Ante | `evals/harbor/prompts/ante/` (capture of 0.2.9), and the docs and changelog in `AntigmaLabs/ante-preview` | `34c8b51` |

**V** means read in source, a doc page or the captured prompt. **I** means
inferred: the source is closed, or nothing was found after a search.

Not reached: `mariozechner.at` (where Pi's Terminal-Bench claim is made) and
`docs.antigma.ai` are outside the network this was run from. Pi's benchmark
claim is unverified; Ante's docs were read from the `docs-site` folder of its
public repository. Claude Code's prompt quotes come from the installed binary,
not from a published prompt.

## Table

| Mechanism | KodAct | AlphaCode | Claude Code | Pi | Ante |
| --- | --- | --- | --- | --- | --- |
| **Check before finishing** | Yes, on by default. A reminder when files changed and no check ran since; once per set of changes; also "the last check failed and nothing changed". `engine.rs` (`verification_reminder`), `verify.rs:26-37`. Off: `KODACT_VERIFY=0`, `verification.enabled`. **V** | Only through todos: a completion-confidence gate and a "confidence spike" re-check, on the model's own scores (`alphacode_base/todo.rs:46-55`). On with auto-poke. **V** | Not built in. The docs say a prompt line "works most of the time, not every time" and point to a user-written `Stop` hook (exit 2 blocks; at most 8 in a row, `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`; off: `disableAllHooks`). **V** | None. **I** | Opt-in `--check`: "runs a second pass after the main task, asking the agent to review its own work" (`docs/usage/headless.mdx`). Off by default. **V** |
| **Diagnostics after edits** | None ("To build" in GOALS, pillar 3). **V** | None found. **I** | Opt-in language-server plugin: "each time Claude edits or writes a file the server handles, Claude gets the errors and warnings". Not in cloud sessions. **V** | None. **I** | None documented. **I** |
| **Check of the final claim** | None. | A claim checker compares the report with the tool history, but only for a mission in its Report phase, and it only logs a warning (`agent/turn_execution.rs:440-483`). **V** | `/goal`: a separate model judges the condition after each turn; "It doesn't run commands or read files independently". Opt-in. **V** | None. **I** | `/goal` with a judge (`CHANGELOG.md`). Opt-in. **V**, internals **I** |
| **Loop guard** | A reminder, once per pattern: the same failure 3 times, the same result 3 times, edits that undo each other, 5 failures in a row (`stuck.rs:22-26`). Always on; no switch found. **V** | A refusal: the third identical failing call is not run, an unknown tool name is refused on its second try, a malformed call on its fourth (`tool/repeat_guard.rs:47-60`, `tool/mod.rs`). Always on. **V** | None documented. **I** | None. **I** | Prompt only: "Do not retry failing commands in a sleep loop; diagnose the root cause" (Bash description, capture). **V** |
| **Empty or reasoning-only reply** | "Act on what you have now", up to 3 in a row, every run (`EMPTY_REPLY_PROMPT`, `ACT_PROMPT`). **V** | Asks for the final answer, up to 5 times, only after tool results (`agent/turn_loops.rs:54`, `agent/response_recovery.rs`). **V** | Retries a tool-use stop that carries no tool call, up to twice (error reference, "Automatic retries"). **V** | None; the turn ends (`packages/agent/src/agent-loop.ts`). **I** | "Handle terminal stop reasons before retrying empty responses" (`CHANGELOG.md`). Details **I** |
| **Open todos, announced next step** | `--autonomous` only, twice each. **V** | Auto-poke, on by default in the TUI and in `run`: "You have N incomplete todos. Continue working, or update the todo tool." Off: `auto_poke = false`, `ALPHACODE_RUN_AUTO_POKE=0`. **V** | None by default. **I** | None. **I** | None documented. **I** |
| **API retry (429, outage)** | Every run since `9560698`: from 5 s, doubling, capped at 60 s, 10 tries, or while time is left with `--max-time`; Esc stops the wait; `KODACT_MAX_RETRIES` caps it (`api_retry_wait`). Under it, 3 quick provider retries. **V** | Every run: 6 tries; 10 s to 180 s on a 429, 5 s base otherwise; `Retry-After` honoured up to 180 s (`agent/response_recovery.rs:217`, `1115-1139`). **V** | Every run: "up to 10 times with exponential backoff". `CLAUDE_CODE_MAX_RETRIES`; without end for unattended runs with `CLAUDE_CODE_RETRY_WATCHDOG` (unset by default). **V** | Every run: `retry.enabled: true`, 3 tries, 2 s base, 60 s cap (`core/settings-defaults.ts:18-26`). Off: `retry.enabled`. **V** | Every run: honours `Retry-After`; a wait over 60 s ends the turn and shows it; a credit-exhaustion 429 is a quota error, not retried (`CHANGELOG.md`). **V**, changelog only |
| **Prompt tells the model to verify** | The detected checks are listed in the environment prompt (GOALS, pillar 3); the wording was not located in source. **I** | "Always verify the result before reporting it"; "Verify what you changed: compile, test, lint, inspect the diff" (`prompt/system_prompt.md:5`, `45`). **V** | "Report outcomes faithfully: if tests fail, say so with the output…" (binary strings). **V** | Nothing. One sentence plus "Be concise" (`core/system-prompt.ts:122`, `156`); the v1.4.0 capture has no "verif" or "test". **V** | "verify the result with the narrowest check that directly exercises it… Do not add a separate review or agent pass by default" (capture, `system.md:28`). **V** |

## Published evidence

None of the four publishes a measured effect for a single mechanism.

- Ante: whole-harness Terminal-Bench 2.1 scores (83.9% with DeepSeek V4.1
  Flash, 370 of 445 trials). A claim, not reproduced here.
- AlphaCode: "historical" speed and memory figures only.
- Claude Code: qualitative guidance only.
- Pi: its evals measure the lift from documentation.
- KodAct: no measured baseline yet (GOALS, priority 1).

## Gaps both ways

Others have, KodAct lacks:
- diagnostics after edits (Claude Code, opt-in);
- refusing a repeated failing call instead of advising (AlphaCode);
- telling a quota 429 from a throttle, and ending the turn on a long
  `Retry-After` (Ante, Claude Code);
- a check of the final claim against the tool history (AlphaCode, log only);
- todo nudges outside unattended runs (AlphaCode).

KodAct has, others lack:
- a finish gate that is on by default and tied to evidence: which files
  changed, which check ran since. Claude Code needs a user hook, Ante a flag,
  AlphaCode relies on the model's own confidence score;
- loop detection that reads results: the same error after changes, edits that
  undo each other, identical output;
- the "last check failed and nothing changed" reminder.

## Proposals, ranked

Each follows `KODACT.md`. Step 5 (measure before and after) is owed for all.

### 1. Retry: tell quota from throttle, and respect a long wait

`api_retry_wait` passes any 429 (`is_retryable`), and clamps `Retry-After` to
60 s. So a 429 for exhausted credit, or one asking for 48 minutes, is tried 10
times a minute apart before the error is shown.

- **Real use:** someone out of credit sees the reason at once instead of
  waiting ten minutes; someone asked to wait an hour is told so.
- **Change:** an error `is_unrecoverable` already recognizes (billing, credit
  balance) is not retried; a `Retry-After` longer than the cap ends the turn
  with the stated wait (unattended runs with `--max-time` keep waiting while
  time is left).
- **Fix, don't fit:** the wait comes from the endpoint and the run's time.
  Nothing depends on the model or its window.
- **Pillar / metric:** failure recovery / recovery rate, wall time.
- **Eval:** mock endpoint, without `--autonomous`: a quota 429 fails on the
  first try; a 429 with a short `Retry-After` then a success passes.

### 2. Report the verification state in the result

- **Real use:** a script or CI job gates on `verified: passed | failed |
  unchecked` instead of trusting the final message. No model call is added.
- **Change:** the result event carries what the tracker already knows.
- **Fix, don't fit:** bookkeeping, the same on every model.
- **Pillar / metric:** verification / false finish. It also shows how often
  "unchecked" goes with a failing checker.
- **Eval:** `fix-median`, `slugify-trap`, `verify-commit`, asserting the field
  against the checker.

### 3. Diagnostics after edits, starting without a language server

- **Real use:** a syntax or type error is seen on the edit, not at the finish
  reminder several turns later.
- **Change:** after an edit, run the project's fast per-file check when it has
  one, and attach only the new errors.
- **Fix, don't fit:** the attached text is capped as a share of the model's
  window; with no error nothing is added, so a strong model is not slowed.
- **Pillar / metric:** verification / turns per task, false finish.
- **Eval:** `rename-symbol`, `split-module`, and a new task where an edit
  breaks a caller in another file.

### 4. Loop guard: refuse after the reminder was ignored

- **Real use:** a weaker model that ignores the reminder stops burning turns
  and money on a call that cannot succeed.
- **Measure first:** count the transcripts where a `repeated_failure` event is
  followed by the same call again. If rare, drop this.
- **Change:** the next identical failing call after the reminder is answered
  with the last error and not run.
- **Fix, don't fit:** keyed on identical input and error, never on time; a
  model that adapts never reaches it.
- **Pillar / metric:** failure recovery / turns wasted after the first error.
- **Eval:** `build-recovery`, `makefile-tabs`.

### 5. Open-todo nudge in print runs without `--autonomous`

- **Real use:** `kodact -p` in a script, where nobody answers, no longer
  returns half the work with todos open.
- **Fix, don't fit:** kept out of the TUI, where stopping to ask is right. The
  same on every model.
- **Pillar / metric:** long-horizon execution / pass rate on multi-step tasks.
- **Eval:** `fix-in-steps`, `steer-more-work`.

### Not proposed

- A second review pass by default. Ante keeps `--check` off, and its prompt
  says not to add one by default: it costs a full pass on every task.
- Confidence gates on the model's own score (AlphaCode): the evidence is the
  model's number, not a result.
