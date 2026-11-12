"""Client 15."""

from svc.net import RETRIES, fetch


def get_15(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/15", retries=retries)
