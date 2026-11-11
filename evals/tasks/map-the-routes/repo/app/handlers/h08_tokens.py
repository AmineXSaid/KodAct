"""Handlers for tokens."""

from app.registry import route


def helper(x):
    return x


# @route("/tokens/8/old")  -- removed in 2.0
def old_tokens(request):
    return None
