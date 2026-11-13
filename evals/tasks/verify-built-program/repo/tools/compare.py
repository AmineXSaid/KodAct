"""Runs ./wc_words on each file in cases/ and compares its count with the .expected file."""

import pathlib
import subprocess
import sys

failed = 0
for case in sorted(pathlib.Path("cases").glob("*.txt")):
    want = case.with_suffix(".expected").read_text().strip()
    got = subprocess.run(["./wc_words", str(case)], capture_output=True, text=True).stdout.strip()
    if got != want:
        print(f"{case.name}: got {got!r}, want {want}")
        failed += 1
print("OK" if not failed else f"{failed} failed")
sys.exit(1 if failed else 0)
