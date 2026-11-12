"""Client 2."""

from svc.net import RETRIES, fetch


def get_2():
    return fetch(
        "/r/2",
        retries=1,
    )
