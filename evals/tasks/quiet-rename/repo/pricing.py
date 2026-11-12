def calc(quantity, unit_price, discount=0.0):
    """The price of a line: quantity times unit price, less a discount (0 to 1)."""
    return round(quantity * unit_price * (1 - discount), 2)
