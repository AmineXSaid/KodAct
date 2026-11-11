cat > lookup.py <<'SOLUTION'
import store

_index = None


def find(key):
    """The value stored for key, or None."""
    global _index
    if _index is None:
        _index = dict(line.partition("=")[::2] for line in store.lines())
    return _index.get(key)
SOLUTION
