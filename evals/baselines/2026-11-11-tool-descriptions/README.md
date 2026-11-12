# Tool descriptions, first measured pass (2026-11-11)

`scripts/eval-descriptions.sh`, KodAct at `502a271`, model `gemini-3.5-flash-lite` through its
OpenAI-compatible endpoint, shell commands in the OS sandbox
(`--sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`), 7 tasks
(the tool-design pillar and `rename-symbol`), 3 runs each: 21 runs a side. `before.md` and
`after.md` are the two reports. Costs read $0: KodAct has no price for this model.

## What was tried

- **TodoWrite:** "a change to one or two files needs no list", and "update the list in the same
  message as your next tool call, not in a message of its own".
- **Glob:** `*.py` matches only directly in the search directory and `**/*.py` at any depth; one
  `**/*` call lists a project, so don't follow it with narrower patterns.

## Result

| Metric | Built-in (A) | Candidates (B) |
| --- | --- | --- |
| Pass rate | 81% | 81% |
| Turns per task | 10.0 | 9.7 |
| Wall time per task | 54.4s | 47.8s |
| False-finish rate | 14% | 19% |
| Invalid-call rate | 5% | 4% |
| TodoWrite calls (all runs) | 12 | 0 |
| Glob calls (all runs) | 35 | 33 |

Neither description is adopted. The pass rate did not move, and 0.3 turns in 21 runs is noise
(docs/BASELINE.md: under about 5 points is no change). The TodoWrite text did what it says, no
list at all on these short tasks, without that showing up as fewer turns; it also removed the
list from `rename-symbol`, a multi-step task, which is not wanted. The Glob text changed nothing.

## What the runs showed besides

- `near-miss-paths` passed 1 run in 6. The model usually found the right files by listing the
  project, then stopped without changing them because the names differed from the request. The
  missing-path error now also names a file with a close name elsewhere below the nearest
  existing directory; 5 further runs with it passed 3. Too few to call, and most of those runs
  never hit the error.
- The lite model's free quota (500 requests a day) ran out during those runs, and KodAct retried
  after a few seconds although the error said "Please retry in 1h36m48s": the wait was only in
  the body, with no `retry-after` header. It is now read from the body.

A second pass needs a model with more quota, and should start from the invalid-call list of a
full-suite run rather than from two tasks' transcripts.
