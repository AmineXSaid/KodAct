import sys

lines = open("data.csv").read().splitlines()
want = [f"b{b}-{i},{b * 100 + i}" for b in (1, 2, 3) for i in (1, 2, 3, 4)]
if lines[:1] != ["id,amount"]:
    sys.exit("data.csv lost its header")
rows = lines[1:]
for w in want:
    n = rows.count(w)
    if n != 1:
        sys.exit(f"row {w!r} is in data.csv {n} times, expected once")
if len(rows) != len(want):
    sys.exit(f"data.csv has {len(rows)} rows, expected {len(want)}")
print("ok")
