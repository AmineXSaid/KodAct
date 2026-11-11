# Baseline (2026-10-06)

## What KodAct is

KodAct is a terminal coding agent written in Rust.
- **Binary:** `kodact`, built from a Cargo workspace of 16 crates.
- **Platforms:** Linux and macOS, from source. Windows is untested.
- **Users:** developers working in a terminal; IDE hosts that drive
  `kodact -p --input-format stream-json`; CI jobs and scripts.
- **Core workflows:**
  1. An interactive session in a project (`kodact`).
  2. A one-shot task (`kodact -p "..."`), whose result goes to stdout as text
     or JSON.
  3. An IDE host over stream-json, with permission prompts.
  4. Resuming or continuing a session.
  5. Measuring the harness (`kodact-eval`).

## Checks run

- **Workspace:** `cargo build`, `cargo clippy --workspace --all-targets` (no
  warnings) and `cargo test --workspace --no-fail-fast`: 127 tests passed.
- **Commands exercised** in an empty `$HOME` with no credentials:
  - `--help`, `--version`;
  - `-p` with text and with json output;
  - an unknown flag;
  - an unreachable endpoint;
  - `doctor`;
  - interactive mode with stdin piped;
  - `--help | head`.
- **GitHub Actions** has never run: every run fails at startup because of an
  account-level block.

## Findings

### Critical: incorrect behaviour, broken automation, unsafe side effects

1. **Errors go to stdout in text mode, and twice.** `kodact -p hi` with no
   credentials prints the error on stderr *and* on stdout. Scripts that
   capture stdout see an error as if it were a result.
2. **One exit code for every failure.** A run that fails, missing credentials
   and a hit limit all exit with `1`, so scripts can't tell them apart. Only
   clap's usage errors are distinct (`2`).
3. **Interactive mode doesn't check for a terminal.** With stdin piped, `kodact`
   starts the interactive session, prints a banner and reads prompts and
   permission answers from the pipe.
4. **`kodact config list` / `get` print secrets.** Values such as API keys in
   the settings `env` block are printed without redaction.
5. **Closed pipes can panic.** Text output uses `println!`, which panics if the
   reader of the pipe exits early.

### Important: confusing workflows, poor errors, missing contracts

6. **Network errors give no cause and no next step.** For example:
   `error sending request for url (…)`.
7. **Missing credentials are found late.** They're reported only at the first
   API call, as an "API Error", with no pointer to `kodact doctor`.
8. **No `--no-input`, `--quiet` or color control (`NO_COLOR`)**, and the
   `--yes` scope isn't documented.
9. **Config and state live in `~/.kodact`.** XDG locations are ignored, and
   there's no command to show where a setting came from.
10. **No written CLI contract:** commands, outputs, exit statuses and config
    precedence aren't specified anywhere.
11. **No shell completions and no release artifacts.**
12. **Shell commands aren't sandboxed.** Safety depends on permission
    prompts alone, so you get many prompts or broad allow rules.

### Optional: convenience and polish

13. Interactive mode is a line-based stand-in until the full-screen UI exists.
14. `doctor` checks little: there's no probe of the endpoint, model or
    sandbox.

## What to keep

- The stream-json protocol and the flag names that the reference CLI's IDE
  hosts depend on. They are public interfaces.
- The engine contracts C1–C11, the settings layers, and the session format.

## Recording the first real baseline

No measured baseline exists yet (docs/GOALS.md, priority 1). It takes a real
API key and about an hour of model time. From the repository root:

1. **Build** the binaries under test:

   ```sh
   cargo build --release
   ```

2. **Check the suite** (no key needed). `validate` runs each task's check on
   its starting files (it must fail) and after its reference solution (it
   must pass); `list` shows the tasks and their pillars:

   ```sh
   target/release/kodact-eval validate
   target/release/kodact-eval list
   ```

3. **Run the baseline**: three runs per task, one fixed model (`--model
   sonnet`), edits and commands allowed without prompts (each task runs in its own scratch copy
   under the output directory, never in this repository):

   ```sh
   export KODACT_API_KEY=...
   target/release/kodact-eval run --label baseline --repeat 3 -- --model sonnet --dangerously-skip-permissions
   ```

   `--only <id|tag|pillar>` runs part of the suite, `-j` sets how many tasks
   run at once (default 2), and `--kodact` points at another `kodact` binary
   (default `target/release/kodact`).

4. **Read the results** in `evals/results/baseline/` (git ignores
   `evals/results/`):
   - `report.md`: pass rate, cost and turns per task, false-finish,
     invalid-call and recovery rates, then a row per pillar and per run;
   - `report.json`: the same, for `compare`;
   - `<task>-<n>/`: each run's `workspace/`, `transcript.jsonl` and
     `stderr.txt`, for finding out why a run failed.

5. **Commit the summary, not the runs.** Copy `report.md` and `report.json`
   to `evals/baselines/<yyyy-mm-dd>-baseline/` and commit them, with the
   KodAct commit and the model in the commit message. Never commit:
   - API keys (check `grep -rn "sk-" evals/baselines/` finds nothing);
   - `transcript.jsonl`, `stderr.txt` or `workspace/`: they hold full model
     traffic, environment details and paths from your machine.

6. **Compare a change** against it: rebuild with the change, run with another
   label, then compare (B relative to A; each argument is a results
   directory or a `report.json`):

   ```sh
   target/release/kodact-eval run --label my-change --repeat 3 -- --model sonnet --dangerously-skip-permissions
   target/release/kodact-eval compare evals/results/baseline evals/results/my-change
   ```

   `scripts/eval-ab.sh --before <commit> --repeat 3 -- --model sonnet --dangerously-skip-permissions`
   does both sides in one go: it builds `<commit>` in a worktree and HEAD here, runs
   HEAD's tasks on each with the same model, window and arguments, and prints this
   table. Each side's directory records the commit and the window in `ab.txt`.

   `scripts/eval-descriptions.sh --prompts <dir> --repeat 3 -- --model sonnet --dangerously-skip-permissions`
   compares one build with and without a set of tool descriptions (`<dir>/tools/<ToolName>.md`),
   which is how a description is tuned: it is kept only if the table says so.

**Reading the A/B report.** `compare` prints one table: pass rate, cost per
task, turns per task, wall time, false-finish rate (the agent said it was
done but the check failed), invalid-call rate (tool calls that errored) and
recovery rate, each for A and B, with "B better", "A better" or "=". With 13
tasks and 3 runs (39 runs a side), a difference of one or two runs is noise:
treat a pass-rate change under about 5 points as no change, and raise
`--repeat` before believing it. A change is a win when the pass rate rises
without the false-finish rate rising, or the cost falls at the same pass
rate.
