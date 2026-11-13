"""Client for the shipping service."""
from clients.base import call

SHIPPING_URL = "http://shipping.internal"
SHIPPING_TIMEOUT = 2  # seconds
SHIPPING_RETRIES = 2


def request(path, payload=None):
    return call(SHIPPING_URL + path, payload, timeout=SHIPPING_TIMEOUT, retries=SHIPPING_RETRIES)
