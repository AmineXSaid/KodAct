from bill import split

assert split(100, 4) == [25, 25, 25, 25]
assert split(0, 3) == [0, 0, 0]
for total, people in [(101, 4), (10, 3), (7, 2)]:
    shares = split(total, people)
    assert len(shares) == people and sum(shares) == total, (total, people, shares)
    assert max(shares) - min(shares) <= 1, (total, people, shares)
print("OK")
