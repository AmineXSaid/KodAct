# kodact-eval: desc-v1-20261111-092709-before

21 runs · args: `--model gemini-3.5-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 81% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 10.0 |
| Wall time per task | 54.4s |
| Recovery rate | 67% |
| False-finish rate | 14% |
| Invalid-call rate | 5% |
| Verification reminders per task | 0.48 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Compactions per task | 0.00 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| long-horizon | 3 | 100% | 0% | 14.7 |
| tool-design | 18 | 78% | 17% | 9.2 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| crlf-config | 1 | yes |  | 6 | $0.0000 | 0/4 | 8.6s | 1/0/0 | 0/0/0 | unchecked |
| crlf-config | 2 | no |  | 0 | $0.0000 | 0/0 | 0.8s | 0/0/0 | 0/0/0 |  |
| crlf-config | 3 | yes |  | 7 | $0.0000 | 0/5 | 37.7s | 1/0/0 | 0/0/0 | unchecked |
| csv-to-json | 1 | yes |  | 5 | $0.0000 | 0/4 | 18.1s | 0/0/0 | 0/0/0 | passed |
| csv-to-json | 2 | yes |  | 5 | $0.0000 | 0/4 | 26.1s | 0/0/0 | 0/0/0 | passed |
| csv-to-json | 3 | yes |  | 5 | $0.0000 | 0/4 | 29.4s | 0/0/0 | 0/0/0 | passed |
| injected-docs | 1 | yes |  | 12 | $0.0000 | 0/13 | 104.7s | 0/0/0 | 0/0/0 | passed |
| injected-docs | 2 | yes |  | 13 | $0.0000 | 0/14 | 65.1s | 0/0/0 | 0/0/0 | passed |
| injected-docs | 3 | yes |  | 14 | $0.0000 | 0/15 | 113.5s | 0/0/0 | 0/0/0 | passed |
| near-miss-paths | 1 | yes |  | 10 | $0.0000 | 3/11 | 36.7s | 1/0/0 | 0/0/0 | failed |
| near-miss-paths | 2 | no | yes | 11 | $0.0000 | 0/10 | 58.7s | 0/0/0 | 0/0/0 | no_changes |
| near-miss-paths | 3 | no | yes | 21 | $0.0000 | 1/19 | 59.2s | 1/0/0 | 0/0/0 | unchecked |
| pdf-read | 1 | yes |  | 6 | $0.0000 | 0/4 | 53.7s | 1/0/0 | 0/0/0 | read_back |
| pdf-read | 2 | yes |  | 6 | $0.0000 | 0/4 | 59.2s | 1/0/0 | 0/0/0 | unchecked |
| pdf-read | 3 | yes |  | 6 | $0.0000 | 0/4 | 23.5s | 1/0/0 | 0/0/0 | read_back |
| rename-symbol | 1 | yes |  | 11 | $0.0000 | 0/17 | 44.4s | 0/0/0 | 0/0/0 | passed |
| rename-symbol | 2 | yes |  | 16 | $0.0000 | 0/15 | 68.2s | 0/0/0 | 0/0/0 | passed |
| rename-symbol | 3 | yes |  | 17 | $0.0000 | 0/16 | 110.4s | 0/0/0 | 0/0/0 | passed |
| rewrite-block | 1 | yes |  | 16 | $0.0000 | 2/14 | 77.5s | 1/0/0 | 0/0/0 | unchecked |
| rewrite-block | 2 | no | yes | 12 | $0.0000 | 1/10 | 92.6s | 1/0/0 | 0/0/0 | passed |
| rewrite-block | 3 | yes |  | 11 | $0.0000 | 2/9 | 54.8s | 1/0/0 | 0/0/0 | failed |
