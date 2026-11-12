# Fix, don't fit

Read this before every fix to KodAct. It is the rule `KODACT.md` sends every
change through, next to "measure, don't assume" in `docs/GOALS.md`.

KodAct runs on whatever model the person chooses. Today that may be a 27B
model behind a company gateway, with a 100k window and a 32k reply cap.
Tomorrow it may be a large model with a window of 200k to 1M tokens and
longer replies. A problem found on one model, in one run or on one benchmark
task is fixed in the harness, for every model. KodAct is never tuned to the
case that showed it.

Before proposing or making a fix, check it against each point:

1. **Limits come from the model.** The context window, the reply cap and the
   time left are read from the model's limits (`modelLimits`, what the
   endpoint reports, the settings) and from the run. Thresholds are shares of
   them. A constant sized for 100k or 32k is a fit.
2. **It holds on a large model.** Say how the fix behaves on a large window
   and a strong, fast model: it helps there or stays out of the way. A fix
   that throttles a model that has room is a fit: lower caps, earlier
   compaction, shorter replies, fewer tokens kept.
3. **It fixes a mechanism, not a task.** The cause is named in what KodAct
   does or fails to do, and the fix applies to any project. No task names,
   file names or benchmark details in code, prompts or tests' logic.
4. **The evidence spans cases.** It is checked across the tasks and runs
   that show the problem, both ways: the failure goes away, and what worked
   still works. One trial is a lead, not a reason.
5. **Tests cover both ends.** Where a limit is involved, the tests run the
   fix with a small and a large window (or reply cap), and the large case
   behaves as before.
6. **Prompts speak to every model.** A prompt rule is good practice for any
   model, worded as such, not a workaround for one model's habits.

7. **Everyday work comes first.** Most sessions are a person at a terminal
   with a small request and no budget or deadline: a question about the
   code, a one-file fix, a rename, a command explained. A change is judged
   there first. What it does for a time-boxed unattended run is a second
   question, and a mechanism that only acts in such runs says so and is not
   counted as helping daily use.
8. **Quiet unless it has something to say.** Every reminder, nudge and hint
   costs tokens on every model and attention on a strong one. KodAct speaks
   when it holds a fact the model does not have (the file no longer parses,
   this check was never run, the call returned nothing), not to give advice
   the model could give itself. Notes due at the same moment go in one
   message, never one model call each. A nudge the model acted on is kept;
   one it did not need is dropped for the run. A nudge is never silenced
   for having worked.
9. **A turn with nothing to report costs nothing.** On the common path (the
   model works, the checks pass) a feature adds no model call, no wait and
   no process. Its proposal states that cost: the tokens it adds to every
   request, and what it adds to a turn where it stays silent, which should
   be zero. Speed is part of the goal, not a side effect.
10. **A guess about what the person meant must fail safe.** Reading a
    request for the files, commands and limits it names is a guess. It acts
    on positive evidence (the request asks to run this), never on the
    absence of a word (no "don't" nearby), and a wrong reading may cost a
    line of text, never a suggestion to do what the person ruled out. Word
    lists in one language are a fit to that language: where the request is
    in another, the mechanism says nothing.

A proposal states, next to its effect in real use, how it behaves on a large
model, and what it costs an everyday turn in which it has nothing to say.

## What KodAct is for

A person's daily companion at the terminal: fast, cheap and right on
ordinary work, with any model. Not a benchmark runner. The same harness has
to serve a strong model, which needs facts and otherwise silence, and a
small one, which needs more help, without knowing which it is running: the
difference comes from what the model does in the run (it stalls, it repeats,
it breaks a file), never from its name or size. A feature that makes a
benchmark task pass and a daily session slower or noisier is a loss.

## How quiet is checked

On tasks a model solves without help, the harness should add nothing. Run
them twice with the same model, once with `reminders: false` and once with
the configuration under test, and compare tokens, model calls and wall time
per task (`kodact-eval compare`): at the same pass rate the two should
match. Any difference is what the configuration costs when it is not
needed, and it is stated with the change. `scripts/quiet-check.sh` does it on
the everyday tasks (tag `quiet`: a question about the code, a one-line fix, a
rename): `scripts/quiet-check.sh --repeat 3 --nudges all -- --model <model>`. The first run is recorded in
`evals/baselines/2026-11-12-quiet/`: the defaults cost about 50% more turns and
tokens than reminders off, and turned a 33% false-finish rate into 0%.

## How it is checked without a large model at hand

`scripts/check-window-scaling.py` runs the real `kodact` binary over HTTP
against a local endpoint that plays the model, once with a 1,000,000-token
window and once with a 100,000-token one. The endpoint refuses, as vLLM
does, any request whose prompt and requested reply exceed its window. No API
key is needed; run it after any change to limits, compaction or requests:

```bash
cargo build -p kodact-cli && scripts/check-window-scaling.py target/debug/kodact
```

It passes when:
- on the large window, nothing is compacted and every request asks for the
  full reply cap, however long the conversation gets within the window;
- on the small window, the conversation is compacted, the requests after it
  carry the person's request word for word, a request near the window asks
  for no more than the room left, and the endpoint refuses nothing;
- `--max-time` stops a slow run near its limit with `stop_reason: "max_time"`.

The run on 2026-11-06, at commit `1bf9a97`: on the 1M window, 7 requests
up to 77,861 tokens, no compaction, all asking 32,000. On the 100k window, the
summary call came at 78,135 tokens and asked 20,532 instead of 32,000, so it
fit; the two requests after the compaction carried the request word for word;
nothing was refused. `--max-time 3` stopped after 3.0 s with `max_time`.

This checks KodAct's behaviour on a large window, not a large model's task
success: that is measured with `kodact-eval` on the model itself.
