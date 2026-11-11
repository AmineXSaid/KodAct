import lookup
import store

keys = [f"item-{i:04d}" for i in range(0, 2000)]
for k in keys:
    assert lookup.find(k) == f"value-{k[5:]}", k
print(f"line reads for {len(keys)} queries: {store.READS}")
