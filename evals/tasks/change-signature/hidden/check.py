import inspect
import subprocess
import sys

sys.path.insert(0, ".")
from shop import pricing
from shop.cart import Cart
from shop.report import daily

params = list(inspect.signature(pricing.total).parameters)
assert params == ["prices", "tax_rate", "discount"], params
assert inspect.signature(pricing.total).parameters["discount"].default is inspect.Parameter.empty, "discount is required"
# The discount comes off the subtotal, before tax.
assert pricing.total([1000], 0.2, 100) == 1080
assert pricing.total([250, 1000], 0.2, 0) == 1500
# Every caller passes one: the cart and the report keep their results.
cart = Cart()
cart.add("pen", 250)
cart.add("ink", 1000)
assert cart.checkout() == 1500, cart.checkout()
assert daily([[250, 1000], [500]]) == ([1500, 600], 2100)
out = subprocess.run([sys.executable, "main.py"], capture_output=True, text=True)
assert out.returncode == 0, out.stderr
print("hidden checks passed")
