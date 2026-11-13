import sys

sys.path.insert(0, ".")
from pricing import line_total, order_total, shipping

assert line_total(250, 4) == 1000
assert line_total(1000, 1, 10) == 900
assert line_total(333, 1, 50) == 167, "half a cent rounds up"
assert line_total(101, 1, 25) == 76
assert line_total(199, 3, 15) == 507
assert [shipping(g) for g in (1, 1000, 1001, 2000, 2001, 2500, 3000)] == [500, 500, 650, 650, 800, 800, 800]
assert order_total([(250, 4, 0), (1000, 1, 10)], 400) == 2400
print("hidden checks passed")
