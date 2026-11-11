"""INVENTORY.json lists every route the app registers, with its handler, and nothing else."""
import json, os, sys

ws = os.getcwd()
sys.path.insert(0, ws)
import app.handlers  # noqa: E402,F401
from app.registry import ROUTES  # noqa: E402

try:
    got = json.load(open(os.path.join(ws, "INVENTORY.json")))
except Exception as e:  # noqa: BLE001
    sys.exit(f"INVENTORY.json is missing or not JSON: {e}")
if not isinstance(got, dict):
    sys.exit("INVENTORY.json must be one object")
missing = sorted(set(ROUTES) - set(got))
extra = sorted(set(got) - set(ROUTES))
wrong = sorted(r for r in set(ROUTES) & set(got) if got[r] != ROUTES[r])
if missing or extra or wrong:
    sys.exit(f"missing {missing[:5]} ({len(missing)}), extra {extra[:5]} ({len(extra)}), wrong handler {wrong[:5]} ({len(wrong)})")
print(f"all {len(ROUTES)} routes")
