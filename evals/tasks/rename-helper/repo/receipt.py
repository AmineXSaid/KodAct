from money import fmt_price


def receipt_lines(items):
    """One line per (name, cents) item, then the total."""
    lines = [f"{name:<20}{fmt_price(cents):>12}" for name, cents in items]
    total = sum(cents for _, cents in items)
    lines.append(f"{'total':<20}{fmt_price(total):>12}")
    return lines
