"""Handlers for users."""

from app.registry import route

PREFIX = "/api/v2"


@route(PREFIX + "/users/40")
def list_users(request):
    return {"users": []}
