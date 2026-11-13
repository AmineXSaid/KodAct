"""Client for the users service."""
from clients.base import call

USERS_URL = "http://users.internal"
USERS_TIMEOUT = 6  # seconds
USERS_RETRIES = 2


def request(path, payload=None):
    return call(USERS_URL + path, payload, timeout=USERS_TIMEOUT, retries=USERS_RETRIES)
