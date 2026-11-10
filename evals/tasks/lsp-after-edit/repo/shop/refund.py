from shop.pricing import format_price as fmt


def refund_notice(order, reason=None):
    if reason is None:
        return f"Refunded {fmt(order['amount'])}"
    # Only disputed orders get here.
    return f"Refunded {fmt(order['amount'])} ({reason}); fee waived: {fmt(order.get('fee', 0))}"
