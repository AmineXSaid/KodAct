"""Order totals, in cents."""


def line_total(unit_cents, quantity, discount_percent=0):
    """Price of one order line after its discount, rounded to the nearest cent."""
    gross = unit_cents * quantity
    return gross * (100 - discount_percent) // 100


def shipping(weight_grams):
    """Flat 500 up to 1 kg, then 150 for each further started kilogram."""
    if weight_grams <= 1000:
        return 500
    return 500 + 150 * ((weight_grams - 1000) // 1000)


def order_total(lines, weight_grams):
    """`lines` is a list of (unit_cents, quantity, discount_percent)."""
    return sum(line_total(*line) for line in lines) + shipping(weight_grams)
