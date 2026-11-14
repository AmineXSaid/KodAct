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
