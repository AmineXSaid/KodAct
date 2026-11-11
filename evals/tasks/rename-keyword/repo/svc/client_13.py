"""Client 13."""

from svc.net import RETRIES, fetch


def get_13():
    return fetch("/r/13", retries=RETRIES + 1, timeout=9)
