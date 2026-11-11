import os
import subprocess
import sys

for cmd in (["python3", "test_lookup.py"], ["python3", "bench.py"]):
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(cmd)} failed:\n{(r.stdout + r.stderr)[-1500:]}")
reads = int(r.stdout.strip().rsplit(" ", 1)[-1])
if reads > 2000:
    sys.exit(f"bench: {reads} line reads, want at most 2000")
# The idea that was tried and found wanting must be gone, every part of it.
left = []
for name in os.listdir("."):
    if name.endswith(".py"):
        text = open(name).read()
        left += [f"{name}: {w}" for w in ("PrefixCache", "PREFIX_CACHE", "PREFIX_LEN", "cache_stats") if w in text]
if left:
    sys.exit("parts of the abandoned idea are still there: " + "; ".join(left))
sys.path.insert(0, ".")
import lookup  # noqa: E402

for key, want in (("item-0042", "value-0042"), ("item-9999", None), ("", None)):
    if lookup.find(key) != want:
        sys.exit(f"find({key!r}) is wrong")
print("ok")
