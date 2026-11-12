# Quiet test, first run (2026-11-12)

An early version of `scripts/quiet-check.sh` (`--repeat 3`), KodAct at `aa3faf4`, model `gemini-3.1-flash-lite` (free
tier), shell commands sandboxed. Three everyday tasks, 9 runs a side:
A with every reminder off (`KODACT_REMINDERS=0`), B with KodAct's defaults (no nudge on).
`silent.md` and `tested.md` are the two reports.

| | A: reminders off | B: defaults |
| --- | --- | --- |
| Pass rate | 67% | 100% |
| False-finish rate | 33% | 0% |
| Turns per task | 9.4 | 14.2 |
| Tokens per task | 75k | 118k |
| Wall time per task | 108s | 96s |

By task:

- **quiet-question** (work out a number, write it to answer.txt): A wrote a wrong number and
  stopped in all three runs (15.5, 15.5, 31.5 for 11.5). In B the verification reminder (the
  answer was never read back or checked) led to a check, and all three were right. This is
  the whole pass-rate difference.
- **quiet-rename** (since dropped as a duplicate of `rename-helper`; rename a function and its callers, no tests): both sides passed 3 of 3. In
  two B runs the reminder said no check had run since the change, and the model imported the
  changed modules once or twice: cost with no gain on this task.
- **quiet-one-line-fix** (since dropped as a duplicate of `fix-page-count`): both passed 3 of 3, no reminder fired in B. B's extra turns there
  (12 against 8.7) come from one 16-turn run: noise at 3 runs.

So KodAct's defaults are not silent on everyday work: about 50% more turns and tokens on these
tasks, bought back on one of the three by a correct answer instead of a false finish. The
reminder speaks from a fact KodAct holds (no check since the last change), as goal.md point 8
asks; what it costs is now measured, and is the number a change to it has to beat.

Owed: more runs (3 a side is a lead), and other models; a strong model may check unprompted,
which would make the reminder silent where it isn't needed.
