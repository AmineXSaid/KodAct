"""Client 23."""

from svc.net import RETRIES, fetch


def get_23(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/23", retries=retries)
