"""Handlers for health."""

from app.registry import route

PREFIX = "/api/v2"


@route(PREFIX + "/health/25")
def list_health(request):
    return {"health": []}
