"""Handlers for teams."""

from app.registry import route

PREFIX = "/api/v2"


@route(PREFIX + "/teams/10")
def list_teams(request):
    return {"teams": []}
