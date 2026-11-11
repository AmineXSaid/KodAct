"""Client 9."""

from svc.net import RETRIES, fetch


def get_9():
    return fetch("/r/9", retries=RETRIES + 1, timeout=9)
