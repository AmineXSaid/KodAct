cat > report.py <<'PY'
import csv
import json
from decimal import Decimal

with open("orders.csv", newline="") as f:
    rows = list(csv.DictReader(f))
cents = sum(int(Decimal(r["amount"].replace("$", "").replace(",", "")) * 100) for r in rows)
with open("report.json", "w") as f:
    json.dump({"count": len(rows), "total": cents}, f)
PY
python3 report.py
