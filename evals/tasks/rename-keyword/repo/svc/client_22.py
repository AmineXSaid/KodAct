"""Client 22."""

from svc.net import RETRIES, fetch


def get_22():
    return fetch(
        "/r/22",
        retries=1,
    )
