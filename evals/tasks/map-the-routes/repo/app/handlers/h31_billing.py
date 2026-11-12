"""Handlers for billing."""

from app.registry import route


@route("/billing/31/status")
def status(request):
    return {"ok": True}
