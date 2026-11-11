"""Client 12."""

from svc.net import RETRIES, fetch


def get_12():
    return fetch("/r/12", retries=2)
