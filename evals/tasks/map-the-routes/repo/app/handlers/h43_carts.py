"""Handlers for carts."""

from app.registry import route


def helper(x):
    return x


# @route("/carts/43/old")  -- removed in 2.0
def old_carts(request):
    return None
