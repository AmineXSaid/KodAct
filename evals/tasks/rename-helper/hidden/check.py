import sys

sys.path.insert(0, ".")
import money  # noqa: E402
from receipt import receipt_lines  # noqa: E402

assert money.format_price(1250) == "12.50 EUR"
assert money.format_price(5, "USD") == "0.05 USD"
assert not hasattr(money, "fmt_price"), "the old name is still defined"
assert receipt_lines([("tea", 350)])[-1].endswith("3.50 EUR")
print("hidden checks passed")
