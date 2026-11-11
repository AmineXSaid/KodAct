"""Client 20."""

from svc.net import RETRIES, fetch


def get_20():
    return fetch("/r/20", retries=2)
