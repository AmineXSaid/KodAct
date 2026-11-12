"""Client 0."""

from svc.net import RETRIES, fetch


def get_0():
    return fetch("/r/0", retries=2)
