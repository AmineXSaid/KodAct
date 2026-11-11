"""Handlers for search."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/search/44/a", "/search/44/b"):
    _make(_p)
