python3 - <<'PY'
import csv
from collections import defaultdict
import openpyxl

by = defaultdict(float)
year = defaultdict(float)
for r in csv.DictReader(open("sales.csv")):
    m = int(r["date"][5:7])
    by[(r["region"], m, r["product"])] += float(r["revenue"])
    year[r["region"]] += float(r["revenue"])
products = ["Basic", "Plus", "Pro", "Enterprise"]
wb = openpyxl.Workbook()
ov = wb.active
ov.title = "Overview"
ov.append(["Region", "Revenue"])
for region in ["North", "South", "East", "West"]:
    ov.append([region, year[region]])
    ws = wb.create_sheet(region)
    ws.append(["Month"] + products)
    for m in range(1, 13):
        ws.append([m] + [by[(region, m, p)] for p in products])
wb.save("sales-summary.xlsx")
PY
