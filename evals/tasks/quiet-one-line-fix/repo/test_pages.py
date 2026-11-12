from pages import page, page_count

assert page_count(20, 10) == 2
assert page_count(21, 10) == 3, page_count(21, 10)
assert page(list(range(21)), 3, 10) == [20]
print("ok")
