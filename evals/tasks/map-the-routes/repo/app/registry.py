"""Where handlers register their routes."""

ROUTES = {}


def route(path):
    """Register the decorated function as the handler of `path`."""

    def register(fn):
        if path in ROUTES:
            raise ValueError(f"route {path} registered twice")
        ROUTES[path] = f"{fn.__module__}.{fn.__name__}"
        return fn

    return register
