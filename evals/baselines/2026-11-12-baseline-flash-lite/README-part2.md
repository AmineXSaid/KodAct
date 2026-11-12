# Baseline, second part (2026-11-12)

The suite's other 23 tasks (long-horizon, context, compatibility), same model and settings as
`README.md`'s first part, KodAct at `d4a6bf4`. `report-part2.md` and `report-part2.json`.

10 of the 23 runs ended on the endpoint's daily quota (500 requests for this model on the free
tier) after the first runs had used it: KodAct stopped each at once with "The endpoint asks to
wait 18h 43m", as it should. They are failures of the run, not of the task, and say nothing
about KodAct. One more (`deliverable-small-window`) could not start: its `--autocompact 32k`
was refused by the CLI; the task now asks for its window with `KODACT_CONTEXT_WINDOW`
(cc222de).

Of the 12 runs that ran to the end, 9 passed. The three that failed:

- `abandon-approach`: the idea the task asks to try and then take out was left in four files.
- `quiet-question`: a wrong number (31.5 for 11.5), with reminders on.
- `serve-after-exit`: nothing answered on the port after the run.

The 47-task baseline is therefore 25 tasks from the first part plus these 12, on one model at
one run each. A full pass owes the 10 quota-stopped tasks and `deliverable-small-window`.
