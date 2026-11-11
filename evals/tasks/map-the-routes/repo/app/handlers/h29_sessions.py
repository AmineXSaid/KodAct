"""Handlers for sessions."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/sessions/29/a", "/sessions/29/b"):
    _make(_p)
