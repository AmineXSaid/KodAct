import lookup

assert lookup.find("item-0000") == "value-0000"
assert lookup.find("item-1999") == "value-1999"
assert lookup.find("item-2000") is None
assert lookup.find("nope") is None
print("ok")
