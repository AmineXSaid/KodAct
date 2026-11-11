"""Client 4."""

from svc.net import RETRIES, fetch


def get_4():
    return fetch("/r/4", retries=2)
