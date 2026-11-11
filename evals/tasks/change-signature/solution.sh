python3 - <<'PY'
import re
p = "shop/pricing.py"
s = open(p).read()
s = s.replace("def total(prices, tax_rate):", "def total(prices, tax_rate, discount):")
s = s.replace("subtotal = sum(prices)", "subtotal = sum(prices) - discount")
open(p, "w").write(s)
for p, old, new in [
    ("shop/cart.py", "self.items], TAX)", "self.items], TAX, 0)"),
    ("shop/report.py", "pricing.total(order, tax_rate)", "pricing.total(order, tax_rate, 0)"),
]:
    s = open(p).read()
    assert old in s
    open(p, "w").write(s.replace(old, new))
PY
