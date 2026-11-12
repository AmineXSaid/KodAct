"""The one network call."""

RETRIES = 3


def fetch(url, retries=RETRIES, timeout=5):
    """Pretend to fetch `url`, trying up to `retries` times."""
    return {"url": url, "tries": retries, "timeout": timeout}
