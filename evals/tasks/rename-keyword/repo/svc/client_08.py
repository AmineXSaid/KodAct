"""Client 8."""

from svc.net import RETRIES, fetch


def get_8():
    return fetch("/r/8", retries=2)
