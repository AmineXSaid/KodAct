# A/B: nudges1-20261112-130005-backOff-off (A) vs nudges1-20261112-130005-backOff-on (B)

| Metric | A | B | |
| --- | --- | --- | --- |
| Pass rate | 100% | 100% | = |
| Cost per task | $0.0000 | $0.0000 | = |
| Turns per task | 10.5 | 11.0 | A better |
| Wall time | 129.0s | 60.7s | B better |
| False-finish rate | 0% | 0% | = |
| Invalid-call rate | 6% | 0% | B better |
| Verification reminders per task | 1.00 | 1.00 | = |
| Loop warnings per task | 0.00 | 0.00 | = |
| Reasoning-only replies per task | 0.00 | 0.00 | = |
| Progress nudges per task | 0.00 | 0.00 | = |
| Tokens per task | 107876 | 93751 | B better |
| Compactions per task | 0.00 | 0.00 | = |
| Failed summaries per task | 0.00 | 0.00 | = |
| Recovery rate | 100% | – | – |

2 runs (A) and 2 runs (B). Differences within a few points are noise at small sample sizes; use `--repeat` to tighten them.

