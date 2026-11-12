"""Handlers for users."""

from app.registry import route

PREFIX = "/api/v1"


@route(PREFIX + "/users/0")
def list_users(request):
    return {"users": []}
