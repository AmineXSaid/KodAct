"""Handlers for exports."""

from app.registry import route

BASE = "/internal"
NAME = "exports-27"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
