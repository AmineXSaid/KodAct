"""Handlers for users."""

from app.registry import route

PREFIX = "/api/v3"


@route(PREFIX + "/users/20")
def list_users(request):
    return {"users": []}
