import sys

sys.path.insert(0, ".")
from shop.invoice import invoice_total
from shop.pricing import format_price
from shop.receipt import receipt_line
from shop.refund import refund_notice

order = {"item": "lamp", "amount": 12.5, "currency": "EUR", "fee": 1}
cases = [
    (format_price(3, "USD"), "3.00 USD"),
    (receipt_line(order), "lamp: 12.50 EUR"),
    (invoice_total({"currency": "GBP", "lines": [{"amount": 1.25}, {"amount": 2}]}), "Total due: 3.25 GBP"),
    (refund_notice(order), "Refunded 12.50 EUR"),
    (refund_notice(order, "damaged"), "Refunded 12.50 EUR (damaged); fee waived: 1.00 EUR"),
]
for got, want in cases:
    if got != want:
        sys.exit(f"got {got!r}, expected {want!r}")
try:
    format_price(1)
except TypeError:
    print("ok")
else:
    sys.exit("currency must be required")
