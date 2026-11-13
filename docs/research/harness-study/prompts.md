# Prompts for the next work

Ready-to-paste prompts for agent sessions that implement the first five
changes from this study. Each prompt stands alone: a fresh session knows
nothing of the study except what is in this folder, and `KODACT.md` loads on
its own, so its rules are not repeated here.

The five changes:

1. **A shorter system prompt.** opencode's base prompt is 756 characters,
   Goose's 1,200; KodAct's is about 6,200 before the unattended section
   (candidate O7).
2. **Edit applies a clear near match.** Several harnesses apply a match that
   differs only in indentation, trailing whitespace or curly quotes, and say
   so. KodAct reports the near match and waits for a retry (A1, G2, C1).
3. **"The new text is already in the file."** opencode and Aider put this in
   the edit error; it ends the most common edit loop (O2).
4. **Compaction protects the prompt cache.** opencode summarizes in the
   background and holds the result until it is needed (O3).
5. **Unattended runs end explicitly, with evidence.** Cline ends a run with a
   tool call carrying a `verified` flag (L3).

There are two ways to run them: one combined prompt, or two prompts run in
order. Either way, tests come first, because the "before" numbers have to be
recorded while the old behaviour is still in place.

## Option A: one prompt, two phases

```text
Read docs/research/harness-study/README.md and the reports it links. Implement
these five changes from the study, test-first:

1. Shorter system prompt. opencode's base prompt is 756 characters, Goose's
   1,200; KodAct's (crates/kodact-engine/prompts/) is about 6,200 before the
   unattended section.
2. Edit near matches. When the exact text is not found, apply a unique match
   that differs only in trailing whitespace, a uniform indentation offset
   (re-indent new_string to the file), or typographic characters (curly quotes,
   Unicode dashes, non-breaking spaces), and say in the result which kind and
   which line. Today Edit reports the near match and waits for a retry.
3. Edit "already applied" hint. On a miss, if new_string is already in the
   file, the error says so first. It ends the most common edit loop.
4. Cache-safe compaction. Before the limit, write the summary in the
   background from the cached prefix and hold it; apply it only when compaction
   is due, keeping the messages since it word for word. The lead and limits are
   shares of the model's window. First check finding N1 in
   07-anthropic-published.md: on models with preserved thinking, do not edit
   earlier messages.
5. Finish tool for unattended runs. In --autonomous runs only, a tool ends the
   run with a summary and a `verified` flag; the result and kodact-eval record
   the claim next to verification.status. Interactive sessions get no new tool.

The ideas come from Apache-2.0 and MIT projects: reimplement from the study's
descriptions, do not copy code, and credit the source in each commit message.

PHASE 1: tests, before any behaviour change
- For each change, add an eval task under evals/tasks/ in the existing format
  (read evals/README.md and three existing tasks first). It must score worse on
  today's build for the reason the change addresses: quiet tasks for the
  prompt; a file with curly quotes, one with tabs, and one with an
  already-applied edit; a long task that compacts on a small window; an
  unattended task where an early finish is tempting.
- Add unit tests for the intended behaviour, #[ignore]d with the change named.
  Compaction tests cover a small and a large window.
- Run `kodact-eval validate`, then record the baseline: `kodact-eval run`, at
  least 3 runs per task, on a small and a large model, plus
  `scripts/quiet-check.sh --repeat 3`. If no API key is set, say the baseline is
  owed and give the exact commands.
- Commit and push before phase 2.

PHASE 2: implementation, one commit per change, in the order 2, 3, 5, 4, 1
- Un-ignore that change's tests and make them pass.
- Run its eval task again and `kodact-eval compare` against the baseline, or
  state that the comparison is owed.
- Changes 1 and 4 alter what the model sees: ship them behind a setting, off by
  default. Turn one on only if the A/B shows equal or better pass rate on both
  models. The new prompt keeps only harness facts and rules about the person's
  own work, each with its reason in one clause.
- In each commit, state the effect in real use, the behaviour on a large
  model, the cost on a turn with nothing to report, and
  "Goals: <pillar> / <metric>". Update docs/GOALS.md, docs/CLI.md and
  docs/PARITY.md where they describe these areas.

Pass the gates in KODACT.md before every push. End with a table: change, eval
task, before/after numbers (or "owed"), on or off by default.
```

## Option B: two prompts, run in order

### B1: tests and baseline

```text
Read docs/research/harness-study/README.md and the reports it links. They list
five changes KodAct will make next:

1. A shorter system prompt. Other harnesses use 756 (opencode) to 3,700 (Cline)
   characters; KodAct's built-in prompt (crates/kodact-engine/prompts/) is about
   6,200 before the unattended section.
2. Edit applies a near match when the only difference is indentation, trailing
   whitespace, or typographic characters (curly quotes, Unicode dashes,
   non-breaking spaces), and says it did. Today Edit reports the near match and
   waits for a retry.
3. Edit's "not found" error says when new_string is already in the file.
4. Compaction protects the prompt cache: the summary is written in the
   background before the limit and held until compaction is due.
5. Unattended runs (--autonomous) end with a finish tool that carries a
   `verified` flag, recorded next to verification.status.

This task builds the tests and records the baseline before any of them is
implemented. Do not change engine or tool behaviour.

For each change:
- Add an eval task under evals/tasks/, in the format of the existing tasks
  (read three of them first, and evals/README.md). It must fail or score worse
  on today's build for the reason the change addresses, and its checker must be
  hidden from the agent. Suggested tasks: quiet tasks for the prompt (reuse the
  `quiet` tag); an edit target whose file uses curly quotes, one with tabs where
  the model will likely send spaces, and one where a retried edit was already
  applied; a long task that reaches compaction on a small window; an unattended
  task where an early finish is tempting.
- Add unit tests that state the intended behaviour and mark them #[ignore] with
  a reason naming the change, so they turn on when it lands. Edit's tests go
  next to crates/kodact-tools/src/builtin/edit.rs; compaction tests must cover a
  small and a large window (docs/goal.md, point 5).
- Run `kodact-eval validate` on the new tasks.

Then record the baseline: `kodact-eval run` on the new tasks plus the existing
quiet tasks, at least 3 runs per task, on one small model and one large model,
and `scripts/quiet-check.sh --repeat 3`. Follow docs/BASELINE.md for where the
results go. If no API key is set, do everything else, say plainly that the
baseline is owed, and give the exact commands to record it.

Pass the gates in KODACT.md, commit with "Goals: <pillar> / <metric>" for each
task, and push. End with a table: change, eval task, unit tests, baseline
numbers (or "owed").
```

### B2: implementation

```text
Read docs/research/harness-study/README.md and the candidate tables in its
reports (O7 in 03-opencode.md; A1 and G2 in 04-aider.md and 02-gemini-cli.md;
C1 in 01-codex.md; O2 in 03-opencode.md; O3 in 03-opencode.md; L3 in
05-cline.md). The eval tasks and ignored unit tests for these changes already
exist; find them with `git log` and evals/tasks/. The code these come from is
Apache-2.0 or MIT: reimplement from the descriptions, do not copy, and credit
the source in each commit message.

Implement the five changes, one commit each, in this order:

1. Edit near matches. When the exact text is not found, try in order: trailing
   whitespace ignored per line; a uniform indentation offset (re-indent
   new_string to match the file); typographic characters normalized (curly
   quotes, Unicode dashes, non-breaking and other Unicode spaces map to ASCII).
   Apply a match only if it is unique; otherwise fail as today. The result names
   which kind of match was applied and the line, so the model knows the file
   differed from what it sent. Keep CRLF handling as it is.
2. Edit "already applied" hint. On a miss, if new_string (trimmed, at least ~20
   characters) is already in the file, say so in the error before any near-match
   hint.
3. Background summary. When the context is within a lead of the compaction
   threshold, start the summary request off the critical path, reusing the
   cached prefix, and hold its result. Apply it only when compaction is due,
   with the messages since it kept word for word. Discard it if the conversation
   has moved too far past it or if it fails twice. The lead and the "too far"
   limit are shares of the model's window, never constants sized for one
   window. Check docs/research/harness-study/07-anthropic-published.md (N1)
   first: if the prefix-check finding is confirmed, the design must not edit
   earlier messages on models with preserved thinking.
4. Finish tool for unattended runs. In --autonomous runs only, add a tool that
   ends the run with a summary and a `verified` boolean. The result records the
   claim beside verification.status, and kodact-eval records both per run, so a
   false finish shows as "claimed verified, no check ran". A reply without the
   tool keeps today's behaviour (it is sent back as now). Interactive sessions
   get no new tool.
5. Shorter system prompt. Write a second built-in prompt set that keeps only
   harness facts, rules about the person's own work (scope, destructive
   actions, commits), and rules the evals show models need. Every rule kept
   gets its reason in one clause. Ship it behind a setting, off by default.
   Turn it on only if the quiet check and the eval suite show equal or better
   pass rate on both a small and a large model.

For each change: un-ignore its unit tests and make them pass, run its eval task
before and after (`kodact-eval run`, then `kodact-eval compare`), and state in
the commit how it behaves on a large model and what it costs a turn with
nothing to report. If no API key is set, the comparison is owed: say so in the
commit and leave the change off by default where it alters what the model sees
(changes 3 and 5). Update docs/GOALS.md, docs/CLI.md and docs/PARITY.md where
they describe these areas.

Pass the gates in KODACT.md before every push. End with a table: change,
tests, before/after numbers (or "owed"), and on/off by default.
```
