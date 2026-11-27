"""sales-summary.xlsx from 50,000 rows: a sheet per region with revenue by
month (rows) and product (columns), and an Overview of each region's year.
Numbers are checked against the data; formulas are recalculated first."""

import csv
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections import defaultdict

import openpyxl

if not os.path.exists("sales-summary.xlsx"):
    sys.exit("sales-summary.xlsx was not made")
try:
    wb = openpyxl.load_workbook("sales-summary.xlsx")
except Exception as e:
    sys.exit(f"sales-summary.xlsx does not open: {e}")
if any(isinstance(c.value, str) and c.value.startswith("=") for ws in wb for row in ws.iter_rows() for c in row):
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "in.xlsx")
        shutil.copy("sales-summary.xlsx", src)
        os.makedirs(os.path.join(tmp, "out"))
        subprocess.run(
            ["soffice", f"-env:UserInstallation=file://{tmp}/profile", "--headless", "--convert-to", "xlsx", "--outdir", os.path.join(tmp, "out"), src],
            capture_output=True, timeout=300,
        )
        wb = openpyxl.load_workbook(os.path.join(tmp, "out", "in.xlsx"), data_only=True)
else:
    wb = openpyxl.load_workbook("sales-summary.xlsx", data_only=True)

by = defaultdict(float)
year = defaultdict(float)
for r in csv.DictReader(open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "sales.csv"))):
    m = int(r["date"][5:7])
    by[(r["region"], m, r["product"])] += float(r["revenue"])
    year[r["region"]] += float(r["revenue"])

names = {n.lower(): n for n in wb.sheetnames}
MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]


def month(v):
    if hasattr(v, "month"):
        return v.month
    if isinstance(v, (int, float)) and 1 <= v <= 12 and float(v).is_integer():
        return int(v)
    if isinstance(v, str):
        s = v.strip().lower()
        m = re.fullmatch(r"(?:\d{4}-)?(\d{1,2})", s)
        if m and 1 <= int(m.group(1)) <= 12:
            return int(m.group(1))
        for i, n in enumerate(MONTHS, 1):
            if s.startswith(n):
                return i
    return None


def near(a, b):
    return isinstance(a, (int, float)) and abs(a - b) <= max(0.5, abs(b) * 1e-6)


for region in ("North", "South", "East", "West"):
    if region.lower() not in names:
        sys.exit(f"no sheet for {region}: {wb.sheetnames}")
    ws = wb[names[region.lower()]]
    grid = [[c.value for c in row] for row in ws.iter_rows()]
    head = next(((i, r) for i, r in enumerate(grid) if sum(isinstance(v, str) and v.strip() in ("Basic", "Plus", "Pro", "Enterprise") for v in r) == 4), None)
    if head is None:
        sys.exit(f"{region}: no header row naming the four products")
    hi, hr = head
    cols = {v.strip(): j for j, v in enumerate(hr) if isinstance(v, str) and v.strip() in ("Basic", "Plus", "Pro", "Enterprise")}
    seen = set()
    for r in grid[hi + 1:]:
        m = next((month(v) for v in r[: min(cols.values())] if month(v)), None)
        if not m:
            continue
        seen.add(m)
        for p, j in cols.items():
            if not near(r[j], by[(region, m, p)]):
                sys.exit(f"{region}, month {m}, {p}: {r[j]!r}, want {by[(region, m, p)]:.0f}")
    if len(seen) != 12:
        sys.exit(f"{region}: want all 12 months, found {sorted(seen)}")

if "overview" not in names:
    sys.exit("no Overview sheet")
ov = [[c.value for c in row] for row in wb[names["overview"]].iter_rows()]
for region, want in year.items():
    row = next((r for r in ov if any(isinstance(v, str) and v.strip() == region for v in r)), None)
    if row is None or not any(near(v, want) for v in row):
        sys.exit(f"Overview: {region}'s yearly revenue should be {want:.0f}: {row}")
print("ok")
