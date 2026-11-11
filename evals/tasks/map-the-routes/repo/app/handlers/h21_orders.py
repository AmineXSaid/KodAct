"""Handlers for orders."""

from app.registry import route


@route("/orders/21/status")
def status(request):
    return {"ok": True}
