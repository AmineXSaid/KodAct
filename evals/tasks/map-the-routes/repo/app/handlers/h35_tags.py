"""Handlers for tags."""

from app.registry import route

PREFIX = "/api/v3"


@route(PREFIX + "/tags/35")
def list_tags(request):
    return {"tags": []}
