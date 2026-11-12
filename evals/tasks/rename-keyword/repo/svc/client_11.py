"""Client 11."""

from svc.net import RETRIES, fetch


def get_11(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/11", retries=retries)
