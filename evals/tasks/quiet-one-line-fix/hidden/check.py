import sys

sys.path.insert(0, ".")
from pages import page_count

for items, per, want in [(0, 10, 0), (1, 10, 1), (10, 10, 1), (11, 10, 2), (99, 7, 15), (5, 1, 5)]:
    if page_count(items, per) != want:
        sys.exit(f"page_count({items}, {per}) = {page_count(items, per)}, want {want}")
print("ok")
