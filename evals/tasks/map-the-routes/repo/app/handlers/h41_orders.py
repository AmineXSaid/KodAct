"""Handlers for orders."""

from app.registry import route


@route("/orders/41/status")
def status(request):
    return {"ok": True}
