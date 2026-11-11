"""The fix, its reproducer and its changelog line: all three were asked for."""
import os, subprocess, sys

ws = os.getcwd()
r = subprocess.run([sys.executable, "run_checks.py"], cwd=ws, capture_output=True, text=True)
if r.returncode != 0:
    sys.exit(f"run_checks.py fails: {r.stdout}{r.stderr}")
sys.path.insert(0, ws)
from parser import parse  # noqa: E402

repro = os.path.join(ws, "repro.txt")
if not os.path.exists(repro):
    sys.exit("repro.txt was not written")
text = open(repro).read()
if text.count("=") < 2:
    sys.exit(f"repro.txt does not hold an input with a second '=': {text!r}")
parse(text)
changes = open(os.path.join(ws, "CHANGES.md")).read().splitlines()
if len([l for l in changes if l.startswith("- ")]) < 2:
    sys.exit("CHANGES.md has no new entry")
print("ok")
