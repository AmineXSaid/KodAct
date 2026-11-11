"""Client 18."""

from svc.net import RETRIES, fetch


def get_18():
    return fetch(
        "/r/18",
        retries=1,
    )
