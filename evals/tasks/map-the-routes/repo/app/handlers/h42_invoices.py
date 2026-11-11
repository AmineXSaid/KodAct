"""Handlers for invoices."""

from app.registry import route

BASE = "/internal"
NAME = "invoices-42"


@route(f"{BASE}/{NAME}")
def handle(request):
    return None
