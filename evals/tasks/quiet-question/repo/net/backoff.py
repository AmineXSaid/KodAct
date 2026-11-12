"""Delays between tries."""

BASE_SECONDS = 0.5
CAP_SECONDS = 4.0


def delay(attempt):
    """Seconds to wait after the given attempt (1 is the first) failed."""
    return min(CAP_SECONDS, BASE_SECONDS * 2 ** (attempt - 1))
