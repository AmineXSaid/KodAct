# kodact-eval: base-31

25 runs · args: `--model gemini-3.1-flash-lite --sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`

| Metric | Value |
| --- | --- |
| Pass rate | 88% |
| Cost per task | $0.0000 |
| Cost per passed task | $0.0000 |
| Turns per task | 11.4 |
| Wall time per task | 145.1s |
| Recovery rate | 83% |
| False-finish rate | 4% |
| Invalid-call rate | 12% |
| Verification reminders per task | 0.36 |
| Loop warnings per task | 0.00 |
| Reasoning-only replies per task | 0.00 |
| Edits told they broke the syntax, per task | 0.04 |
| Progress nudges per task | 0.00 |
| Tokens per task | 131814 |
| Tool calls before the first write | 4.9 |
| Compactions per task | 0.00 |
| Failed summaries per task | 0.00 |

## By pillar

| Pillar | Runs | Pass | False finish | Turns |
| --- | --- | --- | --- | --- |
| recovery | 5 | 100% | 0% | 12.6 |
| tool-design | 7 | 86% | 14% | 12.1 |
| verification | 13 | 85% | 0% | 10.5 |

## Runs

| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Nudges | Tokens | Compactions (done/failed/note) | Changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| answer-file | 1 | yes |  | 8 | $0.0000 | 0/6 | 20.3s | 1/0/0 |  | 186827 | 0/0/0 | unchecked |
| build-recovery | 1 | yes |  | 6 | $0.0000 | 2/5 | 90.1s | 0/0/0 |  | 46320 | 0/0/0 | passed |
| change-signature | 1 | yes |  | 23 | $0.0000 | 3/21 | 361.0s | 1/0/0 |  | 210823 | 0/0/0 | passed |
| crlf-config | 1 | yes |  | 8 | $0.0000 | 1/6 | 93.1s | 1/0/0 |  | 63913 | 0/0/0 | unchecked |
| csv-to-json | 1 | yes |  | 3 | $0.0000 | 0/2 | 15.8s | 0/0/0 |  | 24580 | 0/0/0 | passed |
| encode-for-decoder | 1 | yes |  | 26 | $0.0000 | 2/25 | 477.3s | 0/0/0 |  | 857396 | 0/0/0 | passed |
| exact-output | 1 | yes |  | 5 | $0.0000 | 0/4 | 95.9s | 0/0/0 |  | 61665 | 0/0/0 | passed |
| fix-in-steps | 1 | yes |  | 14 | $0.0000 | 3/13 | 127.6s | 0/0/0 |  | 125035 | 0/0/0 | passed |
| fix-median | 1 | yes |  | 10 | $0.0000 | 2/9 | 124.1s | 0/0/0 |  | 83516 | 0/0/0 | passed |
| format-after-edit | 1 | yes |  | 7 | $0.0000 | 0/6 | 101.3s | 0/0/0 |  | 56031 | 0/0/0 | passed |
| injected-docs | 1 | yes |  | 18 | $0.0000 | 5/17 | 129.9s | 0/0/0 |  | 169838 | 0/0/0 | passed |
| invoice-rounding | 1 | yes |  | 9 | $0.0000 | 1/8 | 123.1s | 0/0/0 |  | 76306 | 0/0/0 | passed |
| makefile-tabs | 1 | yes |  | 5 | $0.0000 | 0/4 | 8.2s | 0/0/0 |  | 39077 | 0/0/0 | passed |
| named-deliverables | 1 | yes |  | 14 | $0.0000 | 1/12 | 168.9s | 1/0/0 |  | 120488 | 0/0/0 | unchecked |
| near-miss-paths | 1 | yes |  | 19 | $0.0000 | 3/17 | 336.3s | 1/0/0 |  | 168533 | 0/0/0 | unchecked |
| pdf-read | 1 | yes |  | 6 | $0.0000 | 0/4 | 160.7s | 1/0/0 |  | 47415 | 0/0/0 | read_back |
| rename-keyword | 1 | no | yes | 16 | $0.0000 | 2/14 | 233.0s | 1/0/0 |  | 149113 | 0/0/0 | failed |
| resume-after-kill | 1 | yes |  | 12 | $0.0000 | 0/12 | 181.6s | 1/0/0 |  | 102509 | 0/0/0 | unchecked |
| rewrite-block | 1 | yes |  | 15 | $0.0000 | 2/13 | 156.7s | 1/0/0 |  | 143146 | 0/0/0 | passed |
| size-limit | 1 | no |  | 24 | $0.0000 | 1/24 | 255.8s | 0/0/0 |  | 255240 | 0/0/0 | unchecked |
| slugify-trap | 1 | yes |  | 8 | $0.0000 | 0/7 | 62.5s | 0/0/0 |  | 66031 | 0/0/0 | passed |
| syntax-after-edit | 1 | yes |  | 7 | $0.0000 | 0/7 | 184.7s | 0/0/0 |  | 58448 | 0/0/0 | unchecked |
| verify-commit | 1 | yes |  | 12 | $0.0000 | 3/11 | 16.0s | 0/0/0 |  | 103537 | 0/0/0 | passed |
| verify-inline | 1 | yes |  | 7 | $0.0000 | 0/7 | 89.2s | 0/0/0 |  | 56535 | 0/0/0 | unchecked |
| verify-program-arg | 1 | no |  | 3 | $0.0000 | 0/3 | 14.0s | 0/0/0 |  | 23028 | 0/0/0 | no_changes |
