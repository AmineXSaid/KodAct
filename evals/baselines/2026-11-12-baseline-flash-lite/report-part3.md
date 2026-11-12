# kodact-eval: base-35rest

11 runs · args: `--model gemini-3.5-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 27% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 4.6 |
| Wall time per task | 74.2s |
| Recovery rate | 11% |
| False-finish rate | 0% |
| Invalid-call rate | 10% |
| Verification reminders per task | 0.09 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Progress nudges per task | 0.00 |
| Tokens per task | 48449 |
| Tool calls before the first write | 8.0 |
| Compactions per task | 0.18 |
| Failed summaries per task | 0.09 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| compatibility | 1 | 0% | 0% | 0.0 |
| context | 3 | 0% | 0% | 7.3 |
| long-horizon | 7 | 43% | 0% | 4.1 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Nudges | Tokens | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| deliverable-small-window | 1 | no |  | 22 | $0.0000 | 4/22 | 270.8s | 0/0/0 |  | 273035 | 2/1/0 | unchecked |
| slow-build | 1 | yes |  | 3 | $0.0000 | 0/7 | 253.1s | 0/0/0 |  | 59179 | 0/0/0 | no_changes |
| split-module | 1 | yes |  | 11 | $0.0000 | 0/10 | 107.8s | 0/0/0 |  | 89913 | 0/0/0 | passed |
| steer-more-work | 1 | yes |  | 6 | $0.0000 | 0/4 | 56.3s | 1/0/0 |  | 44421 | 0/0/0 | unchecked |
| stop-at-target | 1 | no |  | 2 | $0.0000 | 0/2 | 65.2s | 0/0/0 |  | 14238 | 0/0/0 | no_changes |
| switch-idea | 1 | no |  | 6 | $0.0000 | 1/6 | 50.6s | 0/0/0 |  | 45115 | 0/0/0 | failed |
| three-reports | 1 | no |  | 1 | $0.0000 | 0/1 | 10.1s | 0/0/0 |  | 7042 | 0/0/0 | no_changes |
| tool-chain | 1 | no |  | 0 | $0.0000 | 0/0 | 0.5s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
| trace-discount | 1 | no |  | 0 | $0.0000 | 0/0 | 0.6s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
| two-plan-scheduler | 1 | no |  | 0 | $0.0000 | 0/0 | 0.5s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
| verbose-stages | 1 | no |  | 0 | $0.0000 | 0/0 | 0.5s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
