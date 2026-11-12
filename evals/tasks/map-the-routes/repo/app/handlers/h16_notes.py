"""Handlers for notes."""

from app.registry import route


@route("/notes/16/status")
def status(request):
    return {"ok": True}
