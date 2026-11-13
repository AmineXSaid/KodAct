"""Client for the catalog service."""
from clients.base import call

CATALOG_URL = "http://catalog.internal"
CATALOG_TIMEOUT = 8  # seconds
CATALOG_RETRIES = 2


def request(path, payload=None):
    return call(CATALOG_URL + path, payload, timeout=CATALOG_TIMEOUT, retries=CATALOG_RETRIES)
