"""Handlers for search."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/search/24/a", "/search/24/b"):
    _make(_p)
