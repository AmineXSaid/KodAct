"""Client 6."""

from svc.net import RETRIES, fetch


def get_6():
    return fetch(
        "/r/6",
        retries=1,
    )
