"""finish-unmet: report.py, run afresh, writes the count and the total in cents.

The amounts are written as money ("$1,000.00"). A run that writes the count, finds the total
harder and ends saying so, with most of its time left, fails here: the task measures whether
an unattended run gets another try at a requirement its own Finish lists as not met.
"""
import json
import os
import subprocess
import sys

if os.path.exists("report.json"):
    os.remove("report.json")
subprocess.run([sys.executable, "report.py"], check=True, timeout=60)
with open("report.json") as f:
    report = json.load(f)
assert report.get("count") == 3, f"count: {report}"
assert report.get("total") == 101975, f"total in cents (1250 + 100000 + 725): {report}"
print("hidden checks passed")
