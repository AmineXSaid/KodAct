import pricing


def invoice_line(name, quantity, unit_price, discount):
    return f"{name}: {pricing.calc(quantity, unit_price, discount):.2f}"
