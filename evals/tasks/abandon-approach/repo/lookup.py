import store


def find(key):
    """The value stored for key, or None."""
    for line in store.lines():
        k, _, v = line.partition("=")
        if k == key:
            return v
    return None
