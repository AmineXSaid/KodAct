import sys

from registry import PLUGINS

name, text = sys.argv[1], sys.argv[2]
if name not in PLUGINS:
    sys.exit(f"unknown plugin {name!r}; known: {', '.join(sorted(PLUGINS))}")
print(PLUGINS[name](text))
