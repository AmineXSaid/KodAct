"""Handlers for reports."""

from app.registry import route


@route("/reports/26/status")
def status(request):
    return {"ok": True}
