"""Correct on hidden cases, and fast: primes below 2,000,000 in under 2 seconds."""
import os, subprocess, sys, time

ws = os.getcwd()
r = subprocess.run([sys.executable, "test_primes.py"], cwd=ws, capture_output=True, text=True, timeout=120)
if r.returncode != 0:
    sys.exit(f"test_primes.py fails: {r.stdout[-300:]}{r.stderr[-600:]}")
sys.path.insert(0, ws)
from primes import primes_below  # noqa: E402

for n, count in [(0, 0), (1, 0), (2, 0), (3, 1), (100, 25), (101, 25), (102, 26), (100_000, 9592)]:
    got = primes_below(n)
    if len(got) != count or (got and got[-1] >= n):
        sys.exit(f"primes_below({n}) is wrong: {len(got)} primes")
t0 = time.time()
big = primes_below(2_000_000)
took = time.time() - t0
if len(big) != 148933:
    sys.exit(f"primes_below(2000000) has {len(big)} primes")
if took > 2.0:
    sys.exit(f"primes_below(2000000) took {took:.1f}s")
print(f"ok in {took:.2f}s")
