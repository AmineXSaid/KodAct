# kodact-eval: quiet1-20261112-122538-silent

9 runs · args: `--model gemini-3.1-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 67% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 9.4 |
| Wall time per task | 107.8s |
| Recovery rate | 80% |
| False-finish rate | 33% |
| Invalid-call rate | 5% |
| Verification reminders per task | 0.00 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Progress nudges per task | 0.00 |
| Tokens per task | 75173 |
| Tool calls before the first write | 5.7 |
| Compactions per task | 0.00 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| context | 3 | 0% | 100% | 5.3 |
| tool-design | 3 | 100% | 0% | 14.3 |
| verification | 3 | 100% | 0% | 8.7 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Nudges | Tokens | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| quiet-one-line-fix | 1 | yes |  | 8 | $0.0000 | 0/7 | 84.8s | 0/0/0 |  | 61217 | 0/0/0 | passed |
| quiet-one-line-fix | 2 | yes |  | 9 | $0.0000 | 1/8 | 84.2s | 0/0/0 |  | 69879 | 0/0/0 | passed |
| quiet-one-line-fix | 3 | yes |  | 9 | $0.0000 | 1/8 | 92.7s | 0/0/0 |  | 69755 | 0/0/0 | passed |
| quiet-question | 1 | no | yes | 5 | $0.0000 | 0/4 | 4.1s | 0/0/0 |  | 37188 | 0/0/0 | unchecked |
| quiet-question | 2 | no | yes | 5 | $0.0000 | 0/4 | 54.8s | 0/0/0 |  | 37201 | 0/0/0 | unchecked |
| quiet-question | 3 | no | yes | 6 | $0.0000 | 0/5 | 99.8s | 0/0/0 |  | 44674 | 0/0/0 | unchecked |
| quiet-rename | 1 | yes |  | 17 | $0.0000 | 2/16 | 176.7s | 0/0/0 |  | 145456 | 0/0/0 | unchecked |
| quiet-rename | 2 | yes |  | 13 | $0.0000 | 0/12 | 142.9s | 0/0/0 |  | 106166 | 0/0/0 | unchecked |
| quiet-rename | 3 | yes |  | 13 | $0.0000 | 0/12 | 230.4s | 0/0/0 |  | 105025 | 0/0/0 | passed |
