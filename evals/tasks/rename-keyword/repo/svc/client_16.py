"""Client 16."""

from svc.net import RETRIES, fetch


def get_16():
    return fetch("/r/16", retries=2)
