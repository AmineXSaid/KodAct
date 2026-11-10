from shop.pricing import format_price


def receipt_line(order):
    return f"{order['item']}: {format_price(order['amount'])}"
