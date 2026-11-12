"""Handlers for files."""

from app.registry import route


def _make(path):
    @route(path)
    def view(request):
        return path
    return view

for _p in ("/files/34/a", "/files/34/b"):
    _make(_p)
