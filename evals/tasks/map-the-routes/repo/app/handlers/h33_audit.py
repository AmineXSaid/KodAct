"""Handlers for audit."""

from app.registry import route


def helper(x):
    return x


# @route("/audit/33/old")  -- removed in 2.0
def old_audit(request):
    return None
