python3 - <<'PY'
import json
import struct

data = bytes.fromhex(
    "0000403f000000bf000088c00000903f2ef9e43e2ef9643fcdcc4cbe0000c0bf6666663f3333b33eec51383ecdcc0c40"
)
keys = [
    "centre_x", "centre_y", "centre_z", "radius", "light_x", "light_y", "light_z",
    "floor_y", "colour_a", "colour_b", "ambient", "gamma",
]
values = struct.unpack("<12f", data)
with open("scene.json", "w") as f:
    json.dump({k: round(v, 4) for k, v in zip(keys, values)}, f, indent=2)
PY
