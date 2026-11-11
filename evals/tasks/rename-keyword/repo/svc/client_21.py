"""Client 21."""

from svc.net import RETRIES, fetch


def get_21():
    return fetch("/r/21", retries=RETRIES + 1, timeout=9)
