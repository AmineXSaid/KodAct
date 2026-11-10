python3 - <<'PY'
import re
open("shop/pricing.py", "w").write('def format_price(amount, currency):\n    return f"{amount:.2f} {currency}"\n')
p = "shop/receipt.py"; s = open(p).read()
open(p, "w").write(s.replace("format_price(order['amount'])", "format_price(order['amount'], order['currency'])"))
p = "shop/invoice.py"; s = open(p).read()
open(p, "w").write(s.replace("pricing.format_price(total)", 'pricing.format_price(total, invoice["currency"])'))
p = "shop/refund.py"; s = open(p).read()
s = s.replace("fmt(order['amount'])", "fmt(order['amount'], order['currency'])")
s = s.replace("fmt(order.get('fee', 0))", "fmt(order.get('fee', 0), order['currency'])")
open(p, "w").write(s)
PY
