"""Client for the email service."""
from clients.base import call

EMAIL_URL = "http://email.internal"
EMAIL_TIMEOUT = 10  # seconds
EMAIL_RETRIES = 2


def request(path, payload=None):
    return call(EMAIL_URL + path, payload, timeout=EMAIL_TIMEOUT, retries=EMAIL_RETRIES)
