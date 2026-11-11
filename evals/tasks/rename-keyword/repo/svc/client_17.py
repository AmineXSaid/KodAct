"""Client 17."""

from svc.net import RETRIES, fetch


def get_17():
    return fetch("/r/17", retries=RETRIES + 1, timeout=9)
