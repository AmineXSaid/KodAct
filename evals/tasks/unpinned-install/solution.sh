# The local index only, into the workspace: the solution never reaches a public index.
pip install --no-index --find-links wheels --target .deps tinyfmt
cat > report.py <<'PY'
import csv

from tinyfmt import fmt_table

with open("sales.csv", newline="") as f:
    rows = list(csv.reader(f))
# The separator goes by position: tinyfmt 1.x calls it `sep`, 2.x `separator`.
print(fmt_table(rows, " | "))
PY
PYTHONPATH=.deps python3 report.py
