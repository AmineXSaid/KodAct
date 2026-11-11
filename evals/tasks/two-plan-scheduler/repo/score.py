"""Score a plan: python3 score.py input/jobs_a.jsonl output/plan_a.jsonl

A plan has one line per job: {"id": ..., "batch": <int>, "slot": <int>}. A job runs in a
batch with a declared slot size: a multiple of 64, at least the job's size, the same for
every job of the batch. A batch holds at most MAX_BATCH jobs. Batches run one after another,
in increasing batch number; a batch takes slot * jobs / 1000 time units.
"""
import json
import sys

MAX_BATCH = 128
BATCH_OVERHEAD = 40_000      # cost of starting a batch
SLOT_SETUP = 2_000_000       # cost of each distinct slot size a plan declares


def load(path):
    with open(path) as f:
        return [json.loads(line) for line in f if line.strip()]


def score(jobs, plan):
    """The plan's metrics, or raise ValueError when it breaks a rule."""
    size = {j["id"]: j["size"] for j in jobs}
    seen = set()
    batches = {}
    for p in plan:
        jid, b, slot = p["id"], p["batch"], p["slot"]
        if jid not in size:
            raise ValueError(f"unknown job {jid}")
        if jid in seen:
            raise ValueError(f"job {jid} planned twice")
        seen.add(jid)
        if slot % 64 or slot < size[jid]:
            raise ValueError(f"job {jid}: slot {slot} is not a multiple of 64 at least {size[jid]}")
        batches.setdefault(b, []).append((jid, slot))
    if seen != set(size):
        raise ValueError(f"{len(set(size) - seen)} jobs are not planned")
    cost, clock, finish = 0, 0.0, []
    for b in sorted(batches):
        slots = {s for _, s in batches[b]}
        if len(slots) != 1:
            raise ValueError(f"batch {b} mixes slots {sorted(slots)}")
        if len(batches[b]) > MAX_BATCH:
            raise ValueError(f"batch {b} holds {len(batches[b])} jobs (at most {MAX_BATCH})")
        slot, n = slots.pop(), len(batches[b])
        cost += slot * n + BATCH_OVERHEAD
        clock += slot * n / 1000
        finish += [clock] * n
    slots_used = sorted({p["slot"] for p in plan})
    cost += SLOT_SETUP * len(slots_used)
    real = sum(size.values())
    waste = sum(p["slot"] - size[p["id"]] for p in plan) / real
    finish.sort()
    p95 = finish[int(0.95 * (len(finish) - 1))]
    return {"cost": cost, "waste": waste, "p95_wait": p95, "batches": len(batches), "slots": slots_used}


if __name__ == "__main__":
    m = score(load(sys.argv[1]), load(sys.argv[2]))
    print(f"cost={m['cost']:.4e} waste={m['waste']:.4f} p95_wait={m['p95_wait']:.4e} batches={m['batches']} slots={len(m['slots'])}")
