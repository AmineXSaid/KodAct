"""Client 19."""

from svc.net import RETRIES, fetch


def get_19(retries=RETRIES):
    """Take `retries` from the caller."""
    return fetch("/r/19", retries=retries)
