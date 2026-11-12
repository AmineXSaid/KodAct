# kodact-eval: desc-v1-20261111-092709-after

21 runs · args: `--model gemini-3.5-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 81% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 9.7 |
| Wall time per task | 47.8s |
| Recovery rate | 60% |
| False-finish rate | 19% |
| Invalid-call rate | 4% |
| Verification reminders per task | 0.48 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Compactions per task | 0.00 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| long-horizon | 3 | 100% | 0% | 10.3 |
| tool-design | 18 | 78% | 22% | 9.6 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| crlf-config | 1 | yes |  | 6 | $0.0000 | 0/4 | 36.1s | 1/0/0 | 0/0/0 | unchecked |
| crlf-config | 2 | yes |  | 7 | $0.0000 | 0/5 | 18.2s | 1/0/0 | 0/0/0 | unchecked |
| crlf-config | 3 | yes |  | 7 | $0.0000 | 0/5 | 11.9s | 1/0/0 | 0/0/0 | unchecked |
| csv-to-json | 1 | yes |  | 5 | $0.0000 | 0/4 | 5.4s | 0/0/0 | 0/0/0 | passed |
| csv-to-json | 2 | yes |  | 5 | $0.0000 | 0/4 | 22.6s | 0/0/0 | 0/0/0 | passed |
| csv-to-json | 3 | yes |  | 5 | $0.0000 | 0/4 | 61.4s | 0/0/0 | 0/0/0 | passed |
| injected-docs | 1 | no | yes | 1 | $0.0000 | 0/0 | 28.1s | 0/0/0 | 0/0/0 | no_changes |
| injected-docs | 2 | yes |  | 11 | $0.0000 | 0/10 | 58.5s | 0/0/0 | 0/0/0 | passed |
| injected-docs | 3 | yes |  | 12 | $0.0000 | 0/11 | 39.5s | 0/0/0 | 0/0/0 | passed |
| near-miss-paths | 1 | no | yes | 12 | $0.0000 | 1/10 | 82.7s | 1/0/0 | 0/0/0 | unchecked |
| near-miss-paths | 2 | no | yes | 16 | $0.0000 | 0/15 | 66.2s | 0/0/0 | 0/0/0 | passed |
| near-miss-paths | 3 | no | yes | 20 | $0.0000 | 2/18 | 62.4s | 1/0/0 | 0/0/0 | failed |
| pdf-read | 1 | yes |  | 6 | $0.0000 | 0/4 | 5.2s | 1/0/0 | 0/0/0 | unchecked |
| pdf-read | 2 | yes |  | 7 | $0.0000 | 1/5 | 52.3s | 1/0/0 | 0/0/0 | failed |
| pdf-read | 3 | yes |  | 7 | $0.0000 | 0/5 | 12.6s | 1/0/0 | 0/0/0 | unchecked |
| rename-symbol | 1 | yes |  | 8 | $0.0000 | 0/13 | 58.3s | 0/0/0 | 0/0/0 | passed |
| rename-symbol | 2 | yes |  | 10 | $0.0000 | 0/15 | 111.8s | 0/0/0 | 0/0/0 | passed |
| rename-symbol | 3 | yes |  | 13 | $0.0000 | 0/14 | 25.5s | 0/0/0 | 0/0/0 | passed |
| rewrite-block | 1 | yes |  | 15 | $0.0000 | 1/13 | 96.8s | 1/0/0 | 0/0/0 | unchecked |
| rewrite-block | 2 | yes |  | 17 | $0.0000 | 2/15 | 85.6s | 1/0/0 | 0/0/0 | unchecked |
| rewrite-block | 3 | yes |  | 13 | $0.0000 | 0/12 | 61.5s | 0/0/0 | 0/0/0 | passed |
