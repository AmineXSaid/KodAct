"""Client for the storage service."""
from clients.base import call

STORAGE_URL = "http://storage.internal"
STORAGE_TIMEOUT = 12  # seconds
STORAGE_RETRIES = 2


def request(path, payload=None):
    return call(STORAGE_URL + path, payload, timeout=STORAGE_TIMEOUT, retries=STORAGE_RETRIES)
