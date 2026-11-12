# kodact-eval: quiet1-20261112-122538-tested

9 runs · args: `--model gemini-3.1-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 100% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 14.2 |
| Wall time per task | 95.9s |
| Recovery rate | 100% |
| False-finish rate | 0% |
| Invalid-call rate | 6% |
| Verification reminders per task | 0.56 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.00 |
| Progress nudges per task | 0.00 |
| Tokens per task | 117677 |
| Tool calls before the first write | 5.7 |
| Compactions per task | 0.00 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| context | 3 | 100% | 0% | 13.0 |
| tool-design | 3 | 100% | 0% | 17.7 |
| verification | 3 | 100% | 0% | 12.0 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Nudges | Tokens | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| quiet-one-line-fix | 1 | yes |  | 16 | $0.0000 | 2/15 | 112.6s | 0/0/0 |  | 131990 | 0/0/0 | passed |
| quiet-one-line-fix | 2 | yes |  | 11 | $0.0000 | 1/10 | 48.3s | 0/0/0 |  | 87562 | 0/0/0 | passed |
| quiet-one-line-fix | 3 | yes |  | 9 | $0.0000 | 1/8 | 68.1s | 0/0/0 |  | 69807 | 0/0/0 | passed |
| quiet-question | 1 | yes |  | 17 | $0.0000 | 0/15 | 55.9s | 1/0/0 |  | 142092 | 0/0/0 | unchecked |
| quiet-question | 2 | yes |  | 9 | $0.0000 | 0/7 | 48.2s | 1/0/0 |  | 70115 | 0/0/0 | unchecked |
| quiet-question | 3 | yes |  | 13 | $0.0000 | 1/11 | 113.2s | 1/0/0 |  | 107183 | 0/0/0 | unchecked |
| quiet-rename | 1 | yes |  | 15 | $0.0000 | 0/13 | 115.4s | 1/0/0 |  | 123692 | 0/0/0 | passed |
| quiet-rename | 2 | yes |  | 20 | $0.0000 | 1/18 | 126.4s | 1/0/0 |  | 173291 | 0/0/0 | unchecked |
| quiet-rename | 3 | yes |  | 18 | $0.0000 | 1/17 | 175.2s | 0/0/0 |  | 153363 | 0/0/0 | passed |
