"""Client 14."""

from svc.net import RETRIES, fetch


def get_14():
    return fetch(
        "/r/14",
        retries=1,
    )
