# A/B: nudges1-20261112-130005-snapshots-off (A) vs nudges1-20261112-130005-snapshots-on (B)

| Metric | A | B | |
| --- | --- | --- | --- |
| Pass rate | 100% | 100% | = |
| Cost per task | $0.0000 | $0.0000 | = |
| Turns per task | 8.0 | 11.0 | A better |
| Wall time | 11.2s | 107.7s | A better |
| False-finish rate | 0% | 0% | = |
| Invalid-call rate | 13% | 16% | A better |
| Verification reminders per task | 0.00 | 0.50 | A better |
| Loop warnings per task | 0.00 | 0.00 | = |
| Reasoning-only replies per task | 0.00 | 0.00 | = |
| Progress nudges per task | 0.00 | 0.00 | = |
| Tokens per task | 64594 | 92995 | A better |
| Tool calls before the first write | 5.5 | 4.5 | B better |
| Compactions per task | 0.00 | 0.00 | = |
| Failed summaries per task | 0.00 | 0.00 | = |
| Recovery rate | 100% | 100% | = |

2 runs (A) and 2 runs (B). Differences within a few points are noise at small sample sizes; use `--repeat` to tighten them.

