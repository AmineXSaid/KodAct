"""Shared helpers."""

DEPRECATED = []


def deprecated(reason):
    """Mark a function as deprecated, with what to use instead."""

    def mark(fn):
        DEPRECATED.append((fn.__module__, fn.__name__, reason))
        return fn

    return mark
