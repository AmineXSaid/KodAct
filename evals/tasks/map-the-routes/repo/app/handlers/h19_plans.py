"""Handlers for plans."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/plans/19/a", "/plans/19/b"):
    _make(_p)
