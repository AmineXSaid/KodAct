"""Handlers for orders."""

from app.registry import route


@route("/orders/1/status")
def status(request):
    return {"ok": True}
