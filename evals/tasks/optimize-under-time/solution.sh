#!/bin/sh
cat > primes.py <<'PY'
"""Prime numbers."""


def primes_below(n):
    """Every prime p with 2 <= p < n, in order."""
    if n < 3:
        return []
    sieve = bytearray([1]) * n
    sieve[0:2] = b"\x00\x00"
    for p in range(2, int(n ** 0.5) + 1):
        if sieve[p]:
            sieve[p * p::p] = bytearray(len(range(p * p, n, p)))
    return [i for i, ok in enumerate(sieve) if ok]
PY
