"""Append one batch file's rows to data.csv. Loading is slow, and not idempotent."""
import sys
import time

rows = open(sys.argv[1]).read().splitlines()
time.sleep(3)
with open("data.csv", "a") as out:
    out.write("".join(r + "\n" for r in rows))
print(f"loaded {len(rows)} rows from {sys.argv[1]}")
