"""Handlers for exports."""

from app.registry import route

BASE = "/internal"
NAME = "exports-7"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
