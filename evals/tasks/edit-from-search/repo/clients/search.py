"""Client for the search service."""
from clients.base import call

SEARCH_URL = "http://search.internal"
SEARCH_TIMEOUT = 30  # seconds
SEARCH_RETRIES = 2


def request(path, payload=None):
    return call(SEARCH_URL + path, payload, timeout=SEARCH_TIMEOUT, retries=SEARCH_RETRIES)
