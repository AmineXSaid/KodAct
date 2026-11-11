"""Handlers for alerts."""

from app.registry import route

BASE = "/internal"
NAME = "alerts-37"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
