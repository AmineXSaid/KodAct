# Scripted models

Models that know nothing about a task's answer, for measuring the harness
itself when no API key is at hand. A task passes only if KodAct hands the
model what it needs.

## `openai_pdf_reader.py`: the `pdf-read` task on an OpenAI-compatible endpoint

An OpenAI chat-completions endpoint that does three things:
1. reads `budget.pdf` with Read;
2. looks for the Q3 forecast in the tool result;
3. writes it to `answer.txt` only if it finds it there.

```bash
python3 evals/scripted/openai_pdf_reader.py 8791 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8791/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only pdf-read --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-10-13. A is 1610f82, before the OpenAI document fix; B is that fix.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0% | 100% |
| False-finish rate | 100% | 0% |
| Turns per task | 2.0 | 4.0 |

Before the fix, the Read result reached the model as the PDF's base64, so it could never find the figure.

A real-model comparison still needs an API key.

## `openai_stops_install.py`: the `stopped-install` task

A model that starts the install in the background, reads its output once,
stops it with KillShell, writes `skipped` into status.txt and ends. It never
repairs anything, so only KodAct's stop decides what state the package
database is left in. Each reply takes 3 seconds, as a real model's does, so the
stop lands while a package is being set up.

```bash
python3 evals/scripted/openai_stops_install.py 8793 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8793/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only stopped-install --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-14. A is a889576, before the gentle stop; B is the change
that sends an install SIGINT.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0/3 | 3/3 |
| Package database left half-updated | 3/3 | 0/3 |
| False-finish rate | 3/3 | 0/3 |
| Time per run | 18 s | 38 s |

A's KillShell sent SIGTERM: the stand-in died while setting up its first
package. B's sent SIGINT: it finished that package, then stopped. B's runs are
20 seconds longer because KillShell and the end of the session each waited
10 seconds for the install; a stop of anything that isn't an install costs
nothing more.

## `openai_fixes_members.py`: the `named-elsewhere` task

A model that reads src/members.py, writes the fix, runs the tests the request
names and ends. Told that files are missing, it answers that the names were
only words and ends again. So what is measured is what KodAct reports, not what
the model does about it.

```bash
python3 evals/scripted/openai_fixes_members.py 8795 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8795/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
KODACT_NUDGES=constraints kodact-eval run --only named-elsewhere --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-14. A is 46781c1, before the fix; B is the change that
reads a slash between words as words, and a name alone as the file of that
name elsewhere in the project.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 3/3 | 3/3 |
| Files reported missing (`verification.missing`) | `members.py`, `and/or`, `missing/duplicate` in 3/3 | none |
| `constraints` reminders (`KODACT_NUDGES=constraints`) | 1 per run | 0 |
| Turns per run (`KODACT_NUDGES=constraints`) | 5 | 4, as with reminders off |

A told the model "It names files that don't exist: `members.py`, `and/or`,
`missing/duplicate`". The file is src/members.py, and the other two are words.
A real model may then create the files it was told are missing; the task's
check fails a run that does. A real-model comparison still needs an API key.

## `openai_writes_ready.py`: the `dash-prompt` task

A model that writes `ready` into ready.txt and ends. The task's instruction
starts with "- ", so what is measured is whether kodact starts at all.

```bash
python3 evals/scripted/openai_writes_ready.py 8797 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8797/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only dash-prompt --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-19. A is 0f69989, before the fix; B is the fix.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0/3 | 3/3 |
| kodact started | 0/3: `error: unexpected argument '- ' found` (exit 2) | 3/3 |

## `openai_summary_cutoff.py`: the `summary-cutoff` task

A model that does what quick-thinking did on 2026-11-17 (a 100k window).
Asked to summarize the whole conversation, it reasons until the output limit
(`finish_reason: length`) and writes no summary, every time; given its own
analysis back instead, it writes the summary. The server refuses one work
request as overflowing (the vLLM/LiteLLM wording), so KodAct compacts.

```bash
python3 evals/scripted/openai_summary_cutoff.py 8798 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8798/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only summary-cutoff --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-19. A is abc180d, before the change; B is the change that
gives the second summary attempt the first one's analysis in place of the
conversation.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 3/3 | 3/3 |
| Compactions done / failed / KodAct's note | 1 / 2 / 1 | 1 / 1 / 0 |
| Summary written by the model | 0/3 | 3/3 |

Both pass, because the scripted model finishes the task either way. What
changes is what the run keeps after compacting: a summary of the model's own,
instead of KodAct's note, and one attempt that reasons to the output limit
instead of two. On the 2026-11-17 run each such attempt was 20k to 32k tokens
of reasoning, minutes on that endpoint. A real-model comparison still needs
an API key.

A request for the model `summarizer` is answered as a model that doesn't
reason first would: the summary at once. With `compaction.model` it writes
the summaries (A: the session's model alone; B: `KODACT_COMPACTION_MODEL=summarizer`,
both at 2026-11-23's build, `-j 1`, since the server keeps one run's state):

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 3/3 | 3/3 |
| Compactions done / failed / KodAct's note | 1 / 1 / 0 | 1 / 0 / 0 |
| Turns per run | 4 | 4 |

The attempt that reasoned to the output limit is gone: on quick-thinking
that was 20k to 32k tokens and 10 to 15 minutes each. Whether a model that
answers directly writes summaries as useful as the session's own still needs
a real-model comparison.

## `openai_finish_unmet.py`: the `finish-unmet` task

A model that does what 6 deep-thinking runs of 2026-11-16 and 17 did. It
meets one requirement and checks it, then ends with Finish saying another
isn't met, with most of its time left. Here its first report.py writes
`count` and leaves out `total`, because the amounts are written as money
("$1,000.00"). Told which requirements its Finish doesn't show met, it
parses the amounts, runs report.py again and ends with each one met. With
`met` after the port, its first Finish shows every requirement met.

```bash
python3 evals/scripted/openai_finish_unmet.py 8799 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8799/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only finish-unmet --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-19. A is bb17595, before the change; B is the change
that answers a Finish listing a requirement not met.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0/3 | 3/3 |
| False-finish rate | 3/3 | 0/3 |
| Turns per run | 3 | 6 |
| Verification reminders per run | 0 | 1 |
| Turns per run, `met` (every requirement met at the first Finish) | 3 | 3 |

The answer costs one more round where the model's own list says a
requirement isn't met, and nothing where it says all are. The Finish tool
itself grows from about 262 to about 514 tokens in each request of an
unattended run (1,046 → 2,056 characters). Whether real models list their
requirements well, and meet more of them, still needs a real-model
comparison.

## `openai_process_lookup.py`: the `verify-process-lookup` task

A model that does what quick-thinking did on kv-store-grpc (2026-11-23): it
fixes greet.py, runs it (a check that passes), looks for a running copy and
ends. The lookup is `pgrep -x greet.py`, which finds no process of that name
(exit 1); with `missing` after the port it is `procwatch greet.py`, a tool
that isn't installed (exit 127), as pgrep wasn't in that container. Reminded
after its reply, it answers once more.

```bash
python3 evals/scripted/openai_process_lookup.py 8794 &            # or: 8794 missing
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8794/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only verify-process-lookup --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-23. A is 6fac60a, before the change; B is the change that
leaves process lookups and programs that aren't installed out of the checks.

| Metric | A | B |
| --- | --- | --- |
| Pass rate (pgrep / missing) | 3/3 / 3/3 | 3/3 / 3/3 |
| Verification reminders per run | 1 / 1 | 0 / 0 |
| Turns per run | 6 / 6 | 5 / 5 |

A told the model "the last check (`pgrep -x greet.py`) failed" after work it
had checked by running it. `pgrep -f "python3 greet.py"` shows nothing either
way: it matches its own shell's command line and passes.

## `openai_judge_reasons_long.py`: the `judge-reasons-long` task

A model that fixes one of two bugs, runs the visible test (which covers only
that one) and ends with Finish claiming both fixed and `verified`. Told by
KodAct's separate check that the request isn't met, it fixes the second bug,
shows it working and calls Finish again. The same endpoint answers the check
as a reasoning model does: 10,000 tokens of reasoning, then the verdict, from
the same reply budget; a request whose cap is smaller ends while it reasons
(`finish_reason: "length"`, no text). Its verdict reads only the tool results
in the transcript. So what is measured is whether KodAct gives its check room
to answer, not what the model knows.

```bash
python3 evals/scripted/openai_judge_reasons_long.py 8797 &
export KODACT_OPENAI_BASE_URL=http://127.0.0.1:8797/v1 KODACT_OPENAI_API_KEY=sk-test0123456789
kodact-eval run --only judge-reasons-long --kodact <kodact> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-11-23. A is 2733223, before the change; B is the change that
sizes the check's reply from the checking model's own limit.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0/3 | 3/3 |
| Finished as verified, check failed | 3/3 | 0/3 |
| Finish checks that judged nothing | 3/3 | 0/3 |
| Reply cap of the check | 8,000 | 32,000 |
| Turns per run | 4 | 7 |
| Tokens per run | 8,001 | 10,019 |

A's check was cut off while it reasoned, so the false claim stood. B's asked
for the model's reply limit (none is stated for `m`, so KodAct's default of
32,000) and used 10,019 of it; its "not met" sent the model back, and the 3
more turns are the second fix and its check. A check that answers early costs
what it uses: the cap is a ceiling. With the old counter both sides also
showed one "verification reminder" per run, which was the check's own event;
`kodact-eval` now counts checks apart. A real-model comparison still needs an
API key.
