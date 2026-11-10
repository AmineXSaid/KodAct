"""Prices, in cents."""


def total(prices, tax_rate):
    """Sum of the prices plus tax, rounded to the cent."""
    subtotal = sum(prices)
    return subtotal + round(subtotal * tax_rate)
