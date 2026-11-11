"""Context-window discovery, end to end: each case sets the stand-in endpoint's behaviour,
runs the real kodact binary, and checks what it used."""
import json, os, re, subprocess, sys, time, shutil
HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("KODACT_BIN", os.path.join(HERE, "../../target/debug/kodact"))
PORT = int(os.environ.get("KODACT_CTX_PORT", "18090"))
import tempfile
WORK = tempfile.mkdtemp(prefix="kodact-ctx-")
CONFIG, LOG = f"{WORK}/config.json", f"{WORK}/requests.jsonl"

os.makedirs(f"{WORK}/proj", exist_ok=True)

def env_for(case):
    home = f"{WORK}/home-{case['name']}"
    shutil.rmtree(home, ignore_errors=True); os.makedirs(f"{home}/kodact", exist_ok=True)
    if "settings" in case:
        json.dump(case["settings"], open(f"{home}/kodact/settings.json", "w"))
    e = {k: v for k, v in os.environ.items() if not k.startswith(("KODACT_", "ANTHROPIC_", "OPENAI_"))}
    e.update({"HOME": home, "KODACT_HOME": f"{home}/kodact", "TERM": "xterm-256color",
              "KODACT_OPENAI_BASE_URL": case.get("base", f"http://127.0.0.1:{PORT}/v1"),
              "KODACT_MODEL": case.get("model", "probe-model"), "KODACT_OPENAI_API_KEY": "test-key"})
    e.update(case.get("env", {}))
    return e

def setup(case):
    json.dump(case.get("server", {}), open(CONFIG, "w"))
    open(LOG, "w").close()

def requests():
    return [json.loads(l) for l in open(LOG) if l.strip()]

def print_context(case):
    t0 = time.time()
    p = subprocess.run([BIN, "-p", "/context"], cwd=f"{WORK}/proj", env=env_for(case), capture_output=True, text=True, timeout=90, stdin=subprocess.DEVNULL)
    el = time.time() - t0
    out = p.stdout + p.stderr
    w = re.search(r"of ([\d,]+) tokens", out); a = re.search(r"Auto-compact runs at about ([\d,]+)", out)
    num = lambda m: int(m.group(1).replace(",", "")) if m else None
    return {"window": num(w), "compact_at": num(a), "notice": "doesn't know the model" in out, "secs": round(el, 2), "exit": p.returncode, "out": out}

def print_prompt(case, prompt="hello"):
    t0 = time.time()
    p = subprocess.run([BIN, "-p", prompt], cwd=f"{WORK}/proj", env=env_for(case), capture_output=True, text=True, timeout=120, stdin=subprocess.DEVNULL)
    el = time.time() - t0
    posts = [r for r in requests() if r["method"] == "POST"]
    mt = [r.get("max_tokens") or r.get("max_completion_tokens") for r in posts]
    return {"max_tokens": mt, "posts": len(posts), "summaries": sum(r["summary"] for r in posts), "secs": round(el, 2), "exit": p.returncode, "out": (p.stdout + p.stderr)[-600:]}

def tmux(*a): return subprocess.run(["tmux", *a], capture_output=True, text=True).stdout

def tui(case, prompts=("hello",), wait=3.0):
    tmux("kill-session", "-t", "ct")
    e = env_for(case)
    envs = " ".join(f"{k}={json.dumps(v)}" for k, v in e.items() if k.startswith(("KODACT_", "HOME", "TERM")))
    t0 = time.time()
    tmux("new-session", "-d", "-s", "ct", "-x", "120", "-y", "45", f"cd {WORK}/proj && env -i PATH=/usr/bin:/bin {envs} {BIN}; sleep 120")
    ready = None
    while time.time() - t0 < 40:
        s = tmux("capture-pane", "-p", "-t", "ct")
        if "❯" in s and ("Default" in s or "Type" in s or "Ask" in s):
            ready = round(time.time() - t0, 2); break
        time.sleep(0.1)
    screen0 = tmux("capture-pane", "-p", "-t", "ct")
    for p in prompts:
        tmux("send-keys", "-t", "ct", p, "Enter"); time.sleep(wait)
    tmux("send-keys", "-t", "ct", "/context", "Enter"); time.sleep(1.5)
    s = tmux("capture-pane", "-p", "-S", "-200", "-t", "ct")
    tmux("kill-session", "-t", "ct")
    w = re.findall(r"of ([\d,]+) tokens", s); a = re.findall(r"Auto-compact runs at about ([\d,]+)", s)
    num = lambda xs: int(xs[-1].replace(",", "")) if xs else None
    posts = [r for r in requests() if r["method"] == "POST"]
    return {"ready_secs": ready, "window": num(w), "compact_at": num(a), "notice": "doesn't know the model" in (screen0 + s),
            "summaries": sum(r["summary"] for r in posts), "posts": len(posts), "max_tokens": [r.get("max_tokens") or r.get("max_completion_tokens") for r in posts]}

W = 32768
def fields(**f): return {"models": {"fields": f}}
CASES = []
def case(name, mode, server=None, expect=None, **kw): CASES.append({"name": name, "mode": mode, "server": server or {}, "expect": expect or {}, **kw})

# 1. Every field name gateways use, in both modes.
for f, v in [("context_length", W), ("max_model_len", W), ("context_window", W), ("max_context_length", W), ("max_input_tokens", W)]:
    case(f"field-{f}-tui", "tui", fields(**{f: v}), {"window": W, "notice": False})
case("field-top_provider-tui", "tui", fields(top_provider={"context_length": W}), {"window": W})
case("field-model_info-tui", "tui", fields(model_info={"max_input_tokens": W}), {"window": W})
case("field-max_model_len-print", "print_context", fields(max_model_len=W), {"window": W, "notice": False})
case("field-max_model_len-print-prompt", "print_prompt", fields(max_model_len=W), {"max_tokens_le": W - 6000})
# 2. Nothing reported: the guess, said once.
case("none-tui", "tui", fields(), {"window": 1000000, "notice": True})
case("none-print", "print_context", fields(), {"window": 1000000, "notice": True})
# 3. Latency of the model list.
for d in [0.3, 1.0, 2.5]:
    case(f"latency-{d}s-tui", "tui", {"models": {"fields": {"max_model_len": W}, "delay": d}}, {"window": W, "notice": False, "ready_le": d + 2.5})
case("latency-5s-tui", "tui", {"models": {"fields": {"max_model_len": W}, "delay": 5}}, {"window": W, "ready_le": 5.0})
case("latency-30s-tui", "tui", {"models": {"fields": {"max_model_len": W}, "delay": 30}}, {"ready_le": 5.0})
case("latency-2.5s-print", "print_context", {"models": {"fields": {"max_model_len": W}, "delay": 2.5}}, {"window": W})
case("latency-30s-print", "print_context", {"models": {"fields": {"max_model_len": W}, "delay": 30}}, {"secs_le": 6})
case("latency-1s-print-prompt", "print_prompt", {"models": {"fields": {"max_model_len": W}, "delay": 1}}, {"max_tokens_le": W - 6000})
case("slow-chat-tui", "tui", {"models": {"fields": {"max_model_len": W}}, "chat": {"delay": 4}}, {"window": W}, wait=6)
# 4. A broken model list: no crash, the guess, no long wait.
for name, m in [("401", {"status": 401, "raw": '{"error":{"message":"bad key"}}'}), ("404", {"status": 404, "raw": "not found", "ctype": "text/plain"}),
                ("500", {"status": 500, "raw": '{"error":"boom"}'}), ("bad-json", {"raw": "{not json"}), ("html", {"raw": "<html>login</html>", "ctype": "text/html"}),
                ("empty-data", {"raw": '{"data": []}'}), ("no-data-key", {"raw": '{"models": []}'})]:
    case(f"broken-{name}-tui", "tui", {"models": m}, {"window": 1000000, "notice": True, "ready_le": 4.5})
case("refused-tui", "tui", {}, {"window": 1000000, "ready_le": 4.5}, base="http://127.0.0.1:1/v1")
case("refused-print", "print_context", {}, {"window": 1000000, "secs_le": 6}, base="http://127.0.0.1:1/v1")
case("model-not-listed-tui", "tui", {"models": {"omit_model": True, "extra": [{"id": "other", "max_model_len": 8192}]}}, {"window": 1000000})
case("many-models-tui", "tui", {"models": {"fields": {"max_model_len": W}, "extra": [{"id": f"m{i}", "max_model_len": 4096} for i in range(5000)]}}, {"window": W, "ready_le": 4.5})
# 5. Odd values.
for name, v, want in [("zero", 0, 1000000), ("negative", -1, 1000000), ("string", "32768", W), ("float", 32768.0, W),
                      ("huge-10M", 10_000_000, 10_000_000), ("tiny-1000", 1000, 1000)]:
    case(f"value-{name}-tui", "tui", fields(max_model_len=v), {"window": want} if want else {})
# 6. Who wins.
# KODACT_CONTEXT_WINDOW is one value for every model: a window the endpoint reports for the model
# wins over it, so a value set for a small model never shrinks a large one (docs/goal.md).
case("env-and-listing-tui", "tui", fields(max_model_len=W), {"window": W}, env={"KODACT_CONTEXT_WINDOW": "65536"})
case("env-small-listing-large-tui", "tui", fields(max_model_len=1_000_000), {"window": 1_000_000, "notice": False}, env={"KODACT_CONTEXT_WINDOW": "100000"})
case("modelLimits-and-listing-tui", "tui", fields(max_model_len=W), {"window": 100000}, settings={"modelLimits": {"probe-model": {"contextWindow": 100000}}})
# A known model served with a smaller window: the endpoint knows its own deployment.
case("known-model-and-listing-tui", "tui", {"model": "claude-haiku-4-5", "models": {"fields": {"max_model_len": W}}}, {"window": W}, model="claude-haiku-4-5")
case("known-model-and-listing-print-prompt", "print_prompt", {"model": "claude-haiku-4-5", "models": {"fields": {"max_model_len": W}}}, {}, model="claude-haiku-4-5")
case("env-and-listing-print", "print_context", fields(max_model_len=W), {"window": W, "notice": False}, env={"KODACT_CONTEXT_WINDOW": "65536"})
case("env-small-listing-large-print", "print_context", fields(max_model_len=1_000_000), {"window": 1_000_000, "notice": False}, env={"KODACT_CONTEXT_WINDOW": "100000"})
case("env-only-print", "print_context", fields(), {"window": 65536, "notice": False}, env={"KODACT_CONTEXT_WINDOW": "65536"})
case("latency-5s-print-prompt", "print_prompt", {"models": {"fields": {"max_model_len": W}, "delay": 5}}, {"secs_le": 4.5})
case("value-string-print", "print_context", fields(max_model_len="32768"), {"window": W})
# 6b. Learned without a setting: LiteLLM's /model/info, and a refusal that states the window.
case("model-info-print", "print_context", {"models": {"fields": {}, "model_info": {"max_input_tokens": 100000, "max_output_tokens": 32000}}}, {"window": 100000, "notice": False})
case("model-info-large-tui", "tui", {"models": {"fields": {}, "model_info": {"max_input_tokens": 1000000}}}, {"window": 1000000, "notice": False})
# path-tracing, 2026-11-10: no window listed; a 100k model refused 32,000 on a 68,001-token prompt.
# The refusal gives the window and the prompt: the request goes again, fitted, with no compaction.
case("refusal-learned-print-prompt", "print_prompt", {"models": {"fields": {}}, "chat": {"window": 100000, "prompt_tokens": 68001}}, {"max_tokens_eq": [32000, 29999], "summaries": 0, "exit": 0})
# path-tracing, 2026-11-10, later in the run: the window known, the server counts more than KodAct's
# estimate and states only "at least" the window less the reply asked, plus one. Sized to that, the
# request was refused again by the same margin and the run ended; now the reply is halved instead.
case("refusal-at-least-print-prompt", "print_prompt", {"models": {"fields": {}}, "chat": {"window": 100000, "prompt_tokens": 66000, "real_extra": 6000, "at_least": True}}, {"max_tokens_eq": [32000, 29999, 14999], "summaries": 0, "exit": 0})
# A large model that is never refused keeps the whole cap.
case("no-refusal-large-print-prompt", "print_prompt", {"models": {"fields": {}}, "chat": {"window": 1000000, "prompt_tokens": 68001}}, {"max_tokens_eq": [32000], "summaries": 0, "exit": 0})
# 7. The reply limit.
case("max-output-4096-print-prompt", "print_prompt", fields(max_model_len=W, max_output_tokens=4096), {"max_tokens_le": 4096})
case("max-output-over-window-print-prompt", "print_prompt", fields(max_model_len=8192, max_output_tokens=100000), {"max_tokens_le": 8192})
# 8. Compaction on a small window: at ~20k, not every turn.
case("compact-under-threshold-tui", "tui", {"models": {"fields": {"max_model_len": W}}, "chat": {"prompt_tokens": 15000}}, {"summaries": 0}, prompts=("one", "two", "three"))
case("compact-over-threshold-tui", "tui", {"models": {"fields": {"max_model_len": W}}, "chat": {"prompt_tokens": 22000}}, {"summaries_ge": 1}, prompts=("one", "two", "three"))
# 9. The endpoint rejects a too-long prompt.
case("overflow-error-print-prompt", "print_prompt", {"models": {"fields": {}}, "chat": {"error_over": 20000}}, {})

def check(res, exp):
    bad = []
    if "window" in exp and exp["window"] is not None and res.get("window") != exp["window"]: bad.append(f"window {res.get('window')} != {exp['window']}")
    if "notice" in exp and res.get("notice") != exp["notice"]: bad.append(f"notice {res.get('notice')} != {exp['notice']}")
    if "ready_le" in exp and (res.get("ready_secs") is None or res["ready_secs"] > exp["ready_le"]): bad.append(f"ready {res.get('ready_secs')}s > {exp['ready_le']}s")
    if "secs_le" in exp and res.get("secs", 99) > exp["secs_le"]: bad.append(f"took {res.get('secs')}s > {exp['secs_le']}s")
    if "max_tokens_le" in exp:
        mt = [m for m in res.get("max_tokens", []) if m]
        if not mt or max(mt) > exp["max_tokens_le"]: bad.append(f"max_tokens {mt} > {exp['max_tokens_le']}")
    if "summaries" in exp and res.get("summaries") != exp["summaries"]: bad.append(f"summaries {res.get('summaries')} != {exp['summaries']}")
    if "max_tokens_eq" in exp and res.get("max_tokens") != exp["max_tokens_eq"]: bad.append(f"max_tokens {res.get('max_tokens')} != {exp['max_tokens_eq']}")
    if "exit" in exp and res.get("exit") != exp["exit"]: bad.append(f"exit {res.get('exit')} != {exp['exit']}")
    if "summaries_ge" in exp and (res.get("summaries") or 0) < exp["summaries_ge"]: bad.append(f"summaries {res.get('summaries')} < {exp['summaries_ge']}")
    return bad

only = sys.argv[1:]
server = subprocess.Popen([sys.executable, f"{HERE}/server.py", str(PORT), CONFIG, LOG], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(0.5)
import atexit; atexit.register(server.kill)
results = []
for c in CASES:
    if only and not any(o in c["name"] for o in only): continue
    setup(c)
    r = {"tui": lambda: tui(c, c.get("prompts", ("hello",)), c.get("wait", 3.0)), "print_context": lambda: print_context(c), "print_prompt": lambda: print_prompt(c)}[c["mode"]]()
    bad = check(r, c["expect"])
    keep = {k: v for k, v in r.items() if k != "out"}
    print(f"{'PASS' if not bad else 'FAIL'}  {c['name']:<40} {json.dumps(keep)}{'  <-- ' + '; '.join(bad) if bad else ''}", flush=True)
    results.append({"name": c["name"], "ok": not bad, "bad": bad, **r})
json.dump(results, open(f"{WORK}/results.json", "w"), indent=1)
print(f"details: {WORK}/results.json")
print(f"\n{sum(r['ok'] for r in results)}/{len(results)} passed")
sys.exit(0 if all(r["ok"] for r in results) else 1)
