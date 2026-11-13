"""Client for the billing service."""
from clients.base import call

BILLING_URL = "http://billing.internal"
BILLING_TIMEOUT = 5  # seconds
BILLING_RETRIES = 2


def request(path, payload=None):
    return call(BILLING_URL + path, payload, timeout=BILLING_TIMEOUT, retries=BILLING_RETRIES)
