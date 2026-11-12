from primes import primes_below

assert primes_below(2) == []
assert primes_below(3) == [2]
assert primes_below(20) == [2, 3, 5, 7, 11, 13, 17, 19]
assert primes_below(19) == [2, 3, 5, 7, 11, 13, 17]
assert len(primes_below(10_000)) == 1229
print("ok")
