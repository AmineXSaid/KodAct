"""Handlers for tags."""

from app.registry import route

PREFIX = "/api/v1"


@route(PREFIX + "/tags/15")
def list_tags(request):
    return {"tags": []}
