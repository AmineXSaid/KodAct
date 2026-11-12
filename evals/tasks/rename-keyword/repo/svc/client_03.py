"""Client 3."""

from svc.net import RETRIES, fetch


def get_3(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/3", retries=retries)
