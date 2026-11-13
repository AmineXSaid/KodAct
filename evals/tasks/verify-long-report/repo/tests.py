"""The project's check: `python3 tests.py`. Failures are listed first, then a timing report."""
import sys
import time

from pricing import line_total, order_total, shipping

CASES = [
    ("line plain", lambda: line_total(250, 4), 1000),
    ("line 10% off", lambda: line_total(1000, 1, 10), 900),
    ("line rounds half up", lambda: line_total(333, 1, 50), 167),
    ("line rounds down", lambda: line_total(101, 1, 25), 76),
    ("ship light", lambda: shipping(400), 500),
    ("ship exactly 1 kg", lambda: shipping(1000), 500),
    ("ship just over 1 kg", lambda: shipping(1001), 650),
    ("ship exactly 2 kg", lambda: shipping(2000), 650),
    ("ship 2.5 kg", lambda: shipping(2500), 800),
    ("order", lambda: order_total([(250, 4, 0), (1000, 1, 10)], 400), 2400),
]

failed, timings = [], []
for name, run, want in CASES:
    for repeat in range(15):
        start = time.perf_counter()
        got = run()
        timings.append((name, repeat, time.perf_counter() - start))
    if got != want:
        failed.append(f"{name}: got {got}, want {want}")

if failed:
    print(f"FAILED: {len(failed)} of {len(CASES)} cases")
    for line in failed:
        print("  " + line)
else:
    print(f"all {len(CASES)} cases pass")
print()
print("timing report (seconds)")
for name, repeat, seconds in timings:
    print(f"  {name:22} run {repeat:2}  {seconds:.7f}")
print(f"{len(timings)} timed runs")
sys.exit(1 if failed else 0)
