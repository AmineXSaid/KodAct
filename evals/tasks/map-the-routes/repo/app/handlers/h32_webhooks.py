"""Handlers for webhooks."""

from app.registry import route

BASE = "/internal"
NAME = "webhooks-32"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
