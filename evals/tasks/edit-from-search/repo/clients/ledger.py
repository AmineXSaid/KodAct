"""Client for the ledger service."""
from clients.base import call

LEDGER_URL = "http://ledger.internal"
LEDGER_TIMEOUT = 15  # seconds
LEDGER_RETRIES = 2


def request(path, payload=None):
    return call(LEDGER_URL + path, payload, timeout=LEDGER_TIMEOUT, retries=LEDGER_RETRIES)
