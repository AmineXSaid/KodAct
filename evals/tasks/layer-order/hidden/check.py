"""layer-order: the layers are stored in the order their names sort as text (layer0, layer1,
layer10, layer11, layer2, ...). dump.py must read that layout, for model.bin and for another
file of the same layout, and layers.txt and strongest.txt must come from it."""
import os
import random
import struct
import subprocess
import sys
import tempfile

NAMES = [f"layer{i}" for i in range(12)]


def lines(values):
    return [f"{n} weight={values[n][0]:.4f} bias={values[n][1]:.4f} scale={values[n][2]:.4f}" for n in NAMES]


def write(path, values):
    with open(path, "wb") as f:
        for n in sorted(NAMES):
            f.write(struct.pack("<3f", *values[n]))


def dump(path):
    r = subprocess.run([sys.executable, "dump.py", path], capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        sys.exit(f"dump.py {path} failed: {r.stderr.strip()[-500:]}")
    return [ln.strip() for ln in r.stdout.splitlines() if ln.strip()]


scale = [1.5, 0.75, 2.25, 0.5, 1.25, 3.0, 0.25, 2.0, 1.75, 2.5, 3.5, 1.0]
model = {n: (0.125 * (i + 1), 0.5 - 0.25 * i, scale[i]) for i, n in enumerate(NAMES)}
want = lines(model)

for f in ("dump.py", "layers.txt", "strongest.txt"):
    if not os.path.exists(f):
        sys.exit(f"{f} is missing")
got = [ln.strip() for ln in open("layers.txt") if ln.strip()]
if got != want:
    sys.exit(f"layers.txt: {got[:3]}..., want {want[:3]}...")
if open("strongest.txt").read().strip() != "layer10":
    sys.exit(f"strongest.txt: {open('strongest.txt').read().strip()!r}, want 'layer10'")
if dump("model.bin") != want:
    sys.exit("dump.py model.bin doesn't print layers.txt")

# Another export of the same layout: the program reads the layout, not these values.
rng = random.Random(7)
other = {n: tuple(rng.randint(-64, 64) / 16 for _ in range(3)) for n in NAMES}
with tempfile.TemporaryDirectory() as d:
    path = os.path.join(d, "other.bin")
    write(path, other)
    if dump(path) != lines(other):
        sys.exit("dump.py reads another export of the same layout wrong")
print("ok")
