"""Both plans meet their own limits, and together declare at most 6 slot sizes."""
import hashlib, json, os, sys

ws = os.getcwd()
task = os.environ["KODACT_EVAL_TASK_DIR"]
sys.path.insert(0, os.path.join(task, "hidden"))
sys.path.insert(0, ws)
from scorer import score, load  # the scorer as shipped, not the workspace's copy

LIMITS = {
    "a": {"cost": 1.55e7, "waste": 0.53, "p95_wait": 1.6e3},
    "b": {"cost": 1.53e7, "waste": 0.32, "p95_wait": 2.3e3},
}
want = json.load(open(os.path.join(task, "hidden", "inputs.json")))
slots = set()
for n, lim in LIMITS.items():
    path = os.path.join(ws, "input", f"jobs_{n}.jsonl")
    if hashlib.sha256(open(path, "rb").read()).hexdigest() != want[n]:
        sys.exit(f"input/jobs_{n}.jsonl was changed")
    plan_path = os.path.join(ws, "output", f"plan_{n}.jsonl")
    if not os.path.exists(plan_path):
        sys.exit(f"output/plan_{n}.jsonl is missing")
    try:
        m = score(load(path), load(plan_path))
    except (ValueError, KeyError, json.JSONDecodeError) as e:
        sys.exit(f"output/plan_{n}.jsonl is invalid: {e}")
    for k, v in lim.items():
        if m[k] > v:
            sys.exit(f"plan_{n}: {k} {m[k]:.4g} > {v:.4g}")
    slots |= set(m["slots"])
if len(slots) > 6:
    sys.exit(f"the two plans declare {len(slots)} distinct slot sizes together (at most 6)")
print("ok")
