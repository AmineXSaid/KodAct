import json, sys
sys.path.insert(0, ".")  # run from the workspace
from score import score, load, MAX_BATCH
jobs = {n: load(f"input/jobs_{n}.jsonl") for n in "ab"}
r64 = lambda x: -(-x // 64) * 64

def best_slots(sizes, k):
    # DP over distinct rounded sizes: k groups, each served by its max; minimize total padding.
    vals = sorted(set(r64(s) for s in sizes))
    cnt = {}; tot = {}
    for s in sizes:
        v = r64(s); cnt[v] = cnt.get(v, 0) + 1; tot[v] = tot.get(v, 0) + s
    n = len(vals); C = [0]*(n+1); T = [0]*(n+1)
    for i, v in enumerate(vals):
        C[i+1] = C[i] + cnt[v]; T[i+1] = T[i] + tot[v]
    INF = float("inf")
    dp = [[INF]*(n+1) for _ in range(k+1)]; cut = [[0]*(n+1) for _ in range(k+1)]
    dp[0][0] = 0
    for g in range(1, k+1):
        for j in range(1, n+1):
            for i in range(g-1, j):
                c = dp[g-1][i] + vals[j-1]*(C[j]-C[i]) - (T[j]-T[i])
                if c < dp[g][j]:
                    dp[g][j] = c; cut[g][j] = i
    g = min(range(1, k+1), key=lambda g: dp[g][n]); j = n; out = []
    while g:
        out.append(vals[j-1]); j = cut[g][j]; g -= 1
    return sorted(out)

def plan(js, slots, ascending=True):
    by = {}
    for jb in js:
        s = min(x for x in slots if x >= jb["size"]); by.setdefault(s, []).append(jb)
    out, b = [], 0
    for s in sorted(by, reverse=not ascending):
        group = by[s]
        for i in range(0, len(group), MAX_BATCH):
            for jb in group[i:i+MAX_BATCH]:
                out.append({"id": jb["id"], "batch": b, "slot": s})
            b += 1
    return out

def show(label, slots_a, slots_b, ascending=True):
    pa, pb = plan(jobs["a"], slots_a, ascending), plan(jobs["b"], slots_b, ascending)
    ma, mb = score(jobs["a"], pa), score(jobs["b"], pb)
    union = len(set(ma["slots"]) | set(mb["slots"]))
    print(f"{label:34} A cost={ma['cost']:.4e} waste={ma['waste']:.4f} p95={ma['p95_wait']:.4e} | B cost={mb['cost']:.4e} waste={mb['waste']:.4f} p95={mb['p95_wait']:.4e} | union={union}")
    return pa, pb, ma, mb

allsizes = [j["size"] for j in jobs["a"]] + [j["size"] for j in jobs["b"]]
joint = best_slots(allsizes, 6)
res = show("joint DP, 6 slots, small first", joint, joint)
show("joint DP, 6 slots, large first", joint, joint, ascending=False)
show("per-file DP, 6 slots each", best_slots([j["size"] for j in jobs["a"]], 6), best_slots([j["size"] for j in jobs["b"]], 6))
import statistics
qs = sorted(set(r64(int(q)) for q in statistics.quantiles(allsizes, n=6)) | {r64(max(allsizes))})
qs = qs[-6:]
show("quantile slots (6), small first", qs, qs)
even = [r64(int(max(allsizes) * (i + 1) / 6)) for i in range(6)]
show("evenly spaced slots (6)", even, even)
show("joint DP, 5 slots", best_slots(allsizes, 5), best_slots(allsizes, 5))
if len(sys.argv) > 1:
    pa, pb, *_ = res
    for n, p in (("a", pa), ("b", pb)):
        with open(f"output/plan_{n}.jsonl", "w") as f:
            f.writelines(json.dumps(x) + "\n" for x in p)
