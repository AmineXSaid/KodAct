"""Client for the auth service."""
from clients.base import call

AUTH_URL = "http://auth.internal"
AUTH_TIMEOUT = 3  # seconds
AUTH_RETRIES = 2


def request(path, payload=None):
    return call(AUTH_URL + path, payload, timeout=AUTH_TIMEOUT, retries=AUTH_RETRIES)
