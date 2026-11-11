"""Handlers for webhooks."""

from app.registry import route

BASE = "/internal"
NAME = "webhooks-12"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
