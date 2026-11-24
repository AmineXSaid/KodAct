import json
import os
import sys

if not os.path.exists("deck.json"):
    sys.exit("deck.json was not made")
deck = json.load(open("deck.json"))
titles = [s.get("title", "") for s in deck["slides"]]
want = ["Cells", "Feelings", "Helpers"]
missing = [t for t in want if not any(t.lower() in x.lower() for x in titles)]
if missing:
    sys.exit(f"slides missing: {missing}; the deck has {titles}")
if len(deck["slides"]) < 4:
    sys.exit(f"want a title slide and 3 more, got {len(deck['slides'])}")
# Rendered, as asked: Auto runs routine commands (`add`, a render) unasked.
if not os.path.exists(os.path.join("pages", f"slide-{len(deck['slides'])}.txt")):
    sys.exit("the deck was not rendered to pages/")
print("ok")
