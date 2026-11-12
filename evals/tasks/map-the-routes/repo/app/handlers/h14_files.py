"""Handlers for files."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/files/14/a", "/files/14/b"):
    _make(_p)
