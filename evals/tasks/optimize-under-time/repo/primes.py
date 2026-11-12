"""Prime numbers."""


def primes_below(n):
    """Every prime p with 2 <= p < n, in order."""
    out = []
    for k in range(2, n + 1):
        if all(k % d for d in range(2, k)):
            out.append(k)
    return out
