# Nudges, first A/B (2026-11-12)

`scripts/nudge-ab.sh --repeat 2 progress backOff constraints snapshots shellEdit`, KodAct at
`d4a6bf4`, model `gemini-3.5-flash-lite` (free tier), shell commands sandboxed. Each group runs
its own task twice with the nudge off and twice with it on. The four `*.md` files are the A/B
tables (shellEdit has none: one of its runs failed on the endpoint, see below).

| Group (task) | Pass off / on | Turns off → on | Tokens off → on | Nudges given |
| --- | --- | --- | --- | --- |
| progress (map-the-routes) | 2/2, 2/2 | 15.5 → 9.0 | 214k → 99k | 1 (progress_early) |
| backOff (three-reports) | 2/2, 2/2 | 10.5 → 11.0 | 108k → 94k | none |
| constraints (named-deliverables) | 2/2, 2/2 | 12.5 → 9.5 | 102k → 77k | none |
| snapshots (optimize-under-time) | 2/2, 2/2 | 8.0 → 11.0 | 64k → 93k | none told; 2 versions saved per run |
| shellEdit (rename-keyword) | 1/2, 1/1 | 37 → 31 | 513k → 440k | none |

What it shows, and what it doesn't:

- Only one nudge was given in all 20 runs: the early progress nudge, once. Every other
  difference above is between runs in which the nudge never spoke, so it is run-to-run noise:
  the same model on the same task varied from 7 to 13 turns with nothing different. Two runs
  a side cannot tell a nudge's effect from that.
- **The nudges stayed out of the way.** On tasks where nothing went wrong, none fired, and
  back-off had nothing to do. That is the quiet behaviour goal.md asks for, and is the one
  thing these runs establish.
- `progress` is the one lead: the run that got the early nudge wrote its first version at once
  and finished in 7 turns. One run is a lead, not a reason (goal.md point 4).
- `snapshots` saved two passing versions per run and never needed to report one: the cost is
  the git work of saving them, not model calls.
- shellEdit's second "on" run never started: the endpoint answered HTTP 502 ("upstream request
  failed") until the run gave up. Not a KodAct result.

Nothing is turned on by default from this. Owed: runs on tasks where the nudges' conditions
actually arise, and more of them (`--repeat 5` at least), on more than one model.

## Progress, 5 runs a side (same day)

`scripts/nudge-ab.sh --repeat 5 progress`, same model and settings, task `map-the-routes`
(`progress-5x.md`). Every run passed on both sides. Turns 12.0 → 9.0, tokens 139k → 99k,
wall time 95s → 63s with the nudge on.

The nudge spoke in one of the five "on" runs (the early form, once). The four "on" runs it
never spoke in were shorter too (7 to 9 turns, against 9 to 16 with it off), so the
difference is not the nudge's doing: it is the same run-to-run spread as in the 2-run
comparison above. Over both comparisons the nudge fired in 2 of 7 runs where it could have,
and both of those runs passed.

Conclusion for now: on this task and model the progress nudge rarely has anything to say and
costs nothing when silent. Whether it helps when it speaks needs a task where models stall
more often. It stays off by default.

## Since then

The pacing nudges (`progress`, `progressEarly`, `pace`, `backOff`) were removed on 2026-11-13
(CHANGELOG, "The pacing nudges are removed"): Terminal-Bench runs with them on showed no gain
either. The tables above are kept as measured.
