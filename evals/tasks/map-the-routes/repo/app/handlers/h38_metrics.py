"""Handlers for metrics."""

from app.registry import route


def helper(x):
    return x


# @route("/metrics/38/old")  -- removed in 2.0
def old_metrics(request):
    return None
