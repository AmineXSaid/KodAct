import sys

sys.path.insert(0, ".")
import cart
import invoice
import pricing

if hasattr(pricing, "calc"):
    sys.exit("pricing.calc is still there")
if pricing.total_price(2, 1.5, 0.5) != 1.5 or not pricing.total_price.__doc__:
    sys.exit("total_price is wrong or lost its docstring")
if cart.cart_total([(2, 1.5), (1, 4)]) != 7.0:
    sys.exit("cart_total is wrong")
if invoice.invoice_line("pen", 3, 2.0, 0.1) != "pen: 5.40":
    sys.exit("invoice_line is wrong")
for f in ("pricing.py", "cart.py", "invoice.py"):
    if "calc" in open(f).read().replace("total_price", ""):
        sys.exit(f"{f} still says calc")
print("ok")
