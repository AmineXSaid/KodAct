"""Handlers for health."""

from app.registry import route

PREFIX = "/api/v3"


@route(PREFIX + "/health/5")
def list_health(request):
    return {"health": []}
