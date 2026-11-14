import json
import os
import sys

want = len(open("words.txt").read().split())
got = None
if os.path.exists("out/count.txt"):
    got = int(open("out/count.txt").read().strip())
elif os.path.exists("out/count.json"):
    got = json.load(open("out/count.json"))["words"]
if got is None:
    sys.exit("neither out/count.txt nor out/count.json was written")
if got != want:
    sys.exit(f"count is {got}, want {want}")
print("ok")
