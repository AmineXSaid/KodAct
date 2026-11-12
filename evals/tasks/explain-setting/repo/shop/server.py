import os

from shop.settings import DEFAULTS


def listen_address():
    """Where the server listens: the environment first, then the defaults."""
    host = os.environ.get("SHOP_HOST", DEFAULTS["host"])
    port = int(os.environ.get("SHOP_HTTP_PORT", DEFAULTS["port"]))
    return host, port


def worker_count():
    return int(os.environ.get("SHOP_WORKERS", DEFAULTS["workers"]))
