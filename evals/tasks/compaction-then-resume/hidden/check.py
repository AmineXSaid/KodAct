import hashlib
import os
import subprocess
import sys

sys.path.insert(0, os.getcwd())

UNCHANGED = {"build.sh": "6964e9f039972584", "pipeline/runner.py": "6d85eff75baf8d89"}
for path, digest in UNCHANGED.items():
    if hashlib.sha256(open(path, "rb").read()).hexdigest()[:16] != digest:
        sys.exit(f"{path} was changed")
r = subprocess.run(["./build.sh"], capture_output=True, text=True)
if r.returncode != 0 or not r.stdout.rstrip().endswith("build ok"):
    sys.exit((r.stdout + r.stderr)[-3000:])

# Records the build never sees: the fix must hold for them too.
from pipeline import stages  # noqa: E402

lines = [
    "H1,2026-01-01, South ,7.05",
    "H2,2026-01-02,EAST,0.01",
    "H3,2026-01-03,south,100",
    "H4,2026-01-04,  West,19.99",
    "H5,2026-01-05,west ,0.11",
    "H6,2026-01-06,North,0.29",
]
records = [stages.parse(line) for line in lines]
for rec in records:
    stages.normalize(rec)
cents = [rec["cents"] for rec in records]
if cents != [705, 1, 10000, 1999, 11, 29]:
    sys.exit(f"cents {cents}, want [705, 1, 10000, 1999, 11, 29]")
by_region = stages.aggregate(records)
want = {"south": 10705, "east": 1, "west": 2010, "north": 29}
if by_region != want:
    sys.exit(f"aggregate {by_region}, want {want}")
text = stages.report(by_region)
want_text = "east: 0.01\nnorth: 0.29\nsouth: 107.05\nwest: 20.10"
if text != want_text:
    sys.exit(f"report {text!r}, want {want_text!r}")
# The deliverable the instruction names once, at the start: it must survive any compaction.
import tomllib  # noqa: E402

try:
    with open("FIXES.toml", "rb") as f:
        fixes = tomllib.load(f)
except FileNotFoundError:
    sys.exit("FIXES.toml is missing")
except tomllib.TOMLDecodeError as e:
    sys.exit(f"FIXES.toml is not valid TOML: {e}")
want_stages = ["parse", "normalize", "aggregate", "report"]
if fixes.get("stages") != want_stages:
    sys.exit(f"FIXES.toml stages = {fixes.get('stages')!r}, want {want_stages!r}")
print("ok")
