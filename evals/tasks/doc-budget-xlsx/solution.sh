python3 - <<'PY'
import csv
import openpyxl
from openpyxl.chart import BarChart, Reference

wb = openpyxl.Workbook()
data = wb.active
data.title = "Data"
rows = list(csv.reader(open("expenses.csv")))
data.append(rows[0])
for r in rows[1:]:
    data.append([r[0], r[1], r[2], float(r[3])])
n = len(rows)
s = wb.create_sheet("Summary")
s.append(["Category", "Total", "Share"])
cats = sorted({r[1] for r in rows[1:]})
for i, c in enumerate(cats, 2):
    s.append([c, f"=SUMIF(Data!B2:B{n},A{i},Data!D2:D{n})", f"=B{i}/SUM(B2:B{len(cats) + 1})"])
    s[f"C{i}"].number_format = "0.0%"
ch = BarChart()
ch.add_data(Reference(s, min_col=2, min_row=1, max_row=len(cats) + 1), titles_from_data=True)
ch.set_categories(Reference(s, min_col=1, min_row=2, max_row=len(cats) + 1))
s.add_chart(ch, "E2")
wb.save("budget.xlsx")
PY
