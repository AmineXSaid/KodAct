# kodact-eval: base-31b

23 runs · args: `--model gemini-3.1-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 43% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 9.7 |
| Wall time per task | 156.8s |
| Recovery rate | 35% |
| False-finish rate | 13% |
| Invalid-call rate | 9% |
| Verification reminders per task | 0.35 |
| Loop warnings per task | 0.04 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Progress nudges per task | 0.00 |
| Tokens per task | 125991 |
| Tool calls before the first write | 6.0 |
| Compactions per task | 0.09 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| compatibility | 2 | 50% | 0% | 4.0 |
| context | 7 | 43% | 14% | 8.9 |
| long-horizon | 14 | 43% | 14% | 11.0 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Nudges | Tokens | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| abandon-approach | 1 | no | yes | 24 | $0.0000 | 1/23 | 525.0s | 0/1/0 |  | 212083 | 0/0/0 | passed |
| deliverable-after-compaction | 1 | yes |  | 20 | $0.0000 | 4/19 | 394.2s | 0/0/0 |  | 613965 | 1/0/0 | passed |
| deliverable-small-window | 1 | no |  | 0 | $0.0000 | 0/0 | 0.0s | 0/0/0 |  | 0 | 0/0/0 |  |
| env-default | 1 | yes |  | 24 | $0.0000 | 2/22 | 524.3s | 1/0/0 |  | 207684 | 0/0/0 | failed |
| log-needle | 1 | yes |  | 9 | $0.0000 | 1/7 | 130.8s | 1/0/0 |  | 179734 | 0/0/0 | unchecked |
| map-the-routes | 1 | yes |  | 13 | $0.0000 | 0/11 | 237.7s | 1/0/0 |  | 131246 | 0/0/0 | passed |
| notes-across-compaction | 1 | yes |  | 19 | $0.0000 | 4/17 | 242.9s | 1/0/0 |  | 615234 | 1/0/0 | passed |
| optimize-under-time | 1 | yes |  | 16 | $0.0000 | 2/15 | 238.3s | 0/0/0 |  | 136645 | 0/0/0 | passed |
| parallel-plugins | 1 | yes |  | 17 | $0.0000 | 0/23 | 309.5s | 1/0/0 |  | 146587 | 0/0/0 | unchecked |
| quiet-question | 1 | no | yes | 8 | $0.0000 | 0/6 | 22.9s | 1/0/0 |  | 62548 | 0/0/0 | passed |
| reference-skill | 1 | yes |  | 7 | $0.0000 | 0/5 | 85.4s | 1/0/0 |  | 53866 | 0/0/0 | passed |
| rename-symbol | 1 | yes |  | 17 | $0.0000 | 0/16 | 329.3s | 0/0/0 |  | 146511 | 0/0/0 | passed |
| serve-after-exit | 1 | no | yes | 27 | $0.0000 | 5/25 | 141.3s | 1/0/0 |  | 219556 | 0/0/0 | unchecked |
| slow-build | 1 | yes |  | 4 | $0.0000 | 0/4 | 178.3s | 0/0/0 |  | 28776 | 0/0/0 | no_changes |
| split-module | 1 | no |  | 7 | $0.0000 | 0/7 | 108.8s | 0/0/0 |  | 54999 | 0/0/0 | unchecked |
| steer-more-work | 1 | no |  | 7 | $0.0000 | 0/7 | 69.0s | 0/0/0 |  | 52704 | 0/0/0 | unchecked |
| stop-at-target | 1 | no |  | 0 | $0.0000 | 0/0 | 0.6s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
| switch-idea | 1 | no |  | 2 | $0.0000 | 0/2 | 5.6s | 0/0/0 |  | 14234 | 0/0/0 | no_changes |
| three-reports | 1 | no |  | 0 | $0.0000 | 0/0 | 0.5s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
| tool-chain | 1 | no |  | 1 | $0.0000 | 0/1 | 1.4s | 0/0/0 |  | 7053 | 0/0/0 | no_changes |
| trace-discount | 1 | no |  | 1 | $0.0000 | 0/1 | 30.9s | 0/0/0 |  | 7054 | 0/0/0 | no_changes |
| two-plan-scheduler | 1 | no |  | 1 | $0.0000 | 0/1 | 30.1s | 0/0/0 |  | 7321 | 0/0/0 | no_changes |
| verbose-stages | 1 | no |  | 0 | $0.0000 | 0/0 | 0.5s | 0/0/0 |  | 0 | 0/0/0 | no_changes |
