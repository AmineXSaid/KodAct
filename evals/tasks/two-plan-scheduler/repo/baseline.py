"""A working plan, far from good enough: every job alone, in a slot just big enough."""
import json

for name in ("a", "b"):
    with open(f"input/jobs_{name}.jsonl") as f, open(f"output/plan_{name}.jsonl", "w") as out:
        for i, line in enumerate(f):
            job = json.loads(line)
            out.write(json.dumps({"id": job["id"], "batch": i, "slot": -(-job["size"] // 64) * 64}) + "\n")
