import sys

sys.path.insert(0, ".")
from pages import page_count, page_of  # noqa: E402

cases = {(0, 10): 0, (1, 10): 1, (10, 10): 1, (11, 10): 2, (25, 10): 3, (7, 1): 7}
for (items, per), pages in cases.items():
    got = page_count(items, per)
    if got != pages:
        sys.exit(f"page_count({items}, {per}) is {got}, not {pages}")
try:
    page_count(5, 0)
    sys.exit("page_count(5, 0) should raise ValueError")
except ValueError:
    pass
assert page_of(25, 10) == 3
print("hidden checks passed")
