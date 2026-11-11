"""Client 1."""

from svc.net import RETRIES, fetch


def get_1():
    return fetch("/r/1", retries=RETRIES + 1, timeout=9)
