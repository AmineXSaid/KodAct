"""A tiny deck builder, as office tools are: the file first, then one slide per call.

    python3 tools/deck.py init deck.json --title T
    python3 tools/deck.py add deck.json --json '{"layout": "cards", "title": "…", "items": [...]}'
    python3 tools/deck.py render deck.json pages/      (one text "image" per slide)
"""
import json
import os
import sys


def main(argv):
    cmd, path = argv[0], argv[1]
    if cmd == "init":
        title = argv[argv.index("--title") + 1]
        json.dump({"title": title, "slides": [{"layout": "title", "title": title}]}, open(path, "w"))
    elif cmd == "add":
        deck = json.load(open(path))
        deck["slides"].append(json.loads(argv[argv.index("--json") + 1]))
        json.dump(deck, open(path, "w"))
        print(f"added slide {len(deck['slides'])}")
    elif cmd == "render":
        out = argv[2]
        os.makedirs(out, exist_ok=True)
        for i, s in enumerate(json.load(open(path))["slides"], 1):
            open(os.path.join(out, f"slide-{i}.txt"), "w").write(s.get("title", ""))
        print(f"rendered to {out}")
    else:
        sys.exit(f"unknown command {cmd}")


main(sys.argv[1:])
