"""The four stages of the regional sales report."""


def parse(line):
    """One CSV line, `id,date,region,amount`, as a record."""
    rid, date, region, amount = line.split(",")
    return {"id": rid, "date": date, "region": region, "amount": int(amount)}


def normalize(record):
    """Add the amount in cents."""
    record["cents"] = int(record["amount"] * 10)
    return record


def region_key(record):
    """The region a record is counted under."""
    return record["region"].lower()


def aggregate(records):
    """Total cents per region."""
    totals = {}
    for r in records:
        key = region_key(r)
        totals[key] = totals.get(key, 0) + r["cents"]
    return totals


def report(by_region):
    """One line per region, alphabetical: `region: total`, the total in units with two decimals."""
    return "\n".join(f"{region}: {cents / 100}" for region, cents in sorted(by_region.items()))
