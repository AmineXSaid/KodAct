"""Client for the webhooks service."""
from clients.base import call

WEBHOOKS_URL = "http://webhooks.internal"
WEBHOOKS_TIMEOUT = 9  # seconds
WEBHOOKS_RETRIES = 2


def request(path, payload=None):
    return call(WEBHOOKS_URL + path, payload, timeout=WEBHOOKS_TIMEOUT, retries=WEBHOOKS_RETRIES)
