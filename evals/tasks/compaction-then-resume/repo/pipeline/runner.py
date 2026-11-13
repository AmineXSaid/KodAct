"""Run the report pipeline stage by stage, tracing every record; stop at the first failing stage."""

import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from pipeline import stages  # noqa: E402


def fail(stage, message):
    print(f"FAILED stage {stage}: {message}")
    sys.exit(1)


def main():
    with open(os.path.join(ROOT, "data", "expected.json")) as f:
        expected = json.load(f)
    with open(os.path.join(ROOT, "data", "sales.csv")) as f:
        lines = f.read().splitlines()[1:]

    print("== stage 1/4: parse ==")
    records = []
    for n, line in enumerate(lines, 2):
        try:
            r = stages.parse(line)
        except Exception as e:  # noqa: BLE001 - reported with the line that caused it
            fail("parse", f"line {n} {line!r}: {type(e).__name__}: {e}")
        print(f"[parse] line {n:04d} id={r['id']} region={r['region']!r} amount={r['amount']} ok")
        records.append(r)
    if len(records) != expected["records"]:
        fail("parse", f"{len(records)} records, expected {expected['records']}")

    print("== stage 2/4: normalize ==")
    for r in records:
        stages.normalize(r)
        print(f"[normalize] id={r['id']} amount={r['amount']} cents={r['cents']}")
    total = sum(r["cents"] for r in records)
    if total != expected["total_cents"]:
        fail("normalize", f"the records add up to {total} cents, expected {expected['total_cents']}")

    print("== stage 3/4: aggregate ==")
    for r in records:
        print(f"[aggregate] id={r['id']} region={r['region']!r} key={stages.region_key(r)!r}")
    by_region = stages.aggregate(records)
    if sorted(by_region) != sorted(expected["regions"]):
        fail("aggregate", f"regions {sorted(by_region)}, expected {sorted(expected['regions'])}")
    if sum(by_region.values()) != expected["total_cents"]:
        fail("aggregate", f"the regions add up to {sum(by_region.values())} cents, expected {expected['total_cents']}")

    print("== stage 4/4: report ==")
    text = stages.report(by_region)
    for line in text.splitlines():
        print(f"[report] {line}")
    if text != expected["report"]:
        fail("report", f"report differs from data/expected.json:\n{text!r}\nexpected:\n{expected['report']!r}")
    print("build ok")


if __name__ == "__main__":
    main()
