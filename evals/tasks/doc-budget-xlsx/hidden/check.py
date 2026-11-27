"""budget.xlsx: the expenses on Data, and on Summary each category's total and
share as live formulas (they must recalculate to the right numbers), the share
as a percentage, and a chart of the totals."""

import csv
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from collections import defaultdict

import openpyxl

if not os.path.exists("budget.xlsx"):
    sys.exit("budget.xlsx was not made")
try:
    wb = openpyxl.load_workbook("budget.xlsx")
except Exception as e:
    sys.exit(f"budget.xlsx does not open: {e}")
names = {n.lower(): n for n in wb.sheetnames}
if "data" not in names or "summary" not in names:
    sys.exit(f"want sheets Data and Summary, got {wb.sheetnames}")

rows = list(csv.DictReader(open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "expenses.csv"))))
data = wb[names["data"]]
if data.max_row < len(rows) + 1:
    sys.exit(f"Data has {data.max_row} rows; want a header and all {len(rows)} expenses")
total = defaultdict(float)
for r in rows:
    total[r["category"]] += float(r["amount"])
grand = sum(total.values())

# Recalculated by LibreOffice, so a formula is checked by what it computes.
with tempfile.TemporaryDirectory() as tmp:
    src = os.path.join(tmp, "in.xlsx")
    shutil.copy("budget.xlsx", src)
    os.makedirs(os.path.join(tmp, "out"))
    subprocess.run(
        ["soffice", f"-env:UserInstallation=file://{tmp}/profile", "--headless", "--convert-to", "xlsx", "--outdir", os.path.join(tmp, "out"), src],
        capture_output=True, timeout=180,
    )
    out = os.path.join(tmp, "out", "in.xlsx")
    if not os.path.exists(out):
        sys.exit("LibreOffice could not recalculate the workbook")
    calc = openpyxl.load_workbook(out, data_only=True)[names["summary"]]

summary = wb[names["summary"]]
for row in summary.iter_rows():
    for c in row:
        v = calc[c.coordinate].value
        if isinstance(v, str) and v.startswith("#"):
            sys.exit(f"Summary!{c.coordinate} is an error: {v}")

for cat, want in total.items():
    hit = next((c for row in summary.iter_rows() for c in row if isinstance(c.value, str) and c.value.strip() == cat), None)
    if hit is None:
        sys.exit(f"Summary has no row for {cat}")
    cells = [summary.cell(hit.row, col) for col in range(hit.column + 1, summary.max_column + 1)]
    formulas = [c for c in cells if isinstance(c.value, str) and c.value.startswith("=")]
    if not formulas:
        sys.exit(f"{cat}'s total is typed in, not a formula: it won't follow Data")
    got = [calc[c.coordinate].value for c in formulas]
    if not any(isinstance(v, (int, float)) and abs(v - want) < 0.01 for v in got):
        sys.exit(f"{cat}'s total computes to {got}, want {want:.2f}")
    share = [c for c in formulas if isinstance(calc[c.coordinate].value, (int, float)) and abs(calc[c.coordinate].value - want / grand) < 0.0005]
    if not share:
        sys.exit(f"{cat}'s share of the whole ({want / grand:.4f}) is missing or wrong: {got}")
    if "%" not in share[0].number_format:
        sys.exit(f"{cat}'s share is formatted '{share[0].number_format}', not as a percentage")

z = zipfile.ZipFile("budget.xlsx")
if not any(re.match(r"xl/charts/chart\d+\.xml", n) for n in z.namelist()):
    sys.exit("no chart in the workbook")
print("ok")
