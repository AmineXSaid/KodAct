"""Client 7."""

from svc.net import RETRIES, fetch


def get_7(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/7", retries=retries)
