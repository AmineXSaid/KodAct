cat > dump.py <<'PY'
import struct
import sys

names = sorted(f"layer{i}" for i in range(12))
data = open(sys.argv[1], "rb").read()
layers = {n: struct.unpack_from("<3f", data, 12 * k) for k, n in enumerate(names)}
for i in range(12):
    w, b, s = layers[f"layer{i}"]
    print(f"layer{i} weight={w:.4f} bias={b:.4f} scale={s:.4f}")
PY
python3 dump.py model.bin > layers.txt
python3 - <<'PY'
best = max((float(ln.split("scale=")[1]), ln.split()[0]) for ln in open("layers.txt"))
open("strongest.txt", "w").write(best[1] + "\n")
PY
