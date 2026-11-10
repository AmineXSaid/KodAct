from shop import pricing


def daily(orders, tax_rate=0.2):
    """One total per order, then the day's total."""
    per_order = [pricing.total(order, tax_rate) for order in orders]
    return per_order, sum(per_order)
