"""Handlers for teams."""

from app.registry import route

PREFIX = "/api/v1"


@route(PREFIX + "/teams/30")
def list_teams(request):
    return {"teams": []}
