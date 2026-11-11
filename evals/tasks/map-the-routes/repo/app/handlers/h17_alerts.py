"""Handlers for alerts."""

from app.registry import route

BASE = "/internal"
NAME = "alerts-17"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
