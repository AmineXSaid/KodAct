"""Client 5."""

from svc.net import RETRIES, fetch


def get_5():
    return fetch("/r/5", retries=RETRIES + 1, timeout=9)
