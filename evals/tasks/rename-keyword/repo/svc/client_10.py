"""Client 10."""

from svc.net import RETRIES, fetch


def get_10():
    return fetch(
        "/r/10",
        retries=1,
    )
