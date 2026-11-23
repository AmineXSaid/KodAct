from shuffle import riffle

assert riffle([]) == []
assert riffle([1, 2]) == [1, 2]
assert riffle([1, 2, 3, 4]) == [1, 3, 2, 4]
assert riffle(list("abcdef")) == list("adbecf")
print("ok")
