#!/usr/bin/env python3
"""Check, end to end and without an API key, that KodAct sizes its limits from the model's window.

The real `kodact` binary runs the same scripted session twice against a local OpenAI-compatible
endpoint that plays the model: once as a model with a large window (1,000,000 tokens), once with a
small one (100,000). The endpoint refuses, as vLLM does, any request whose prompt and requested
reply add up to more than its window. The session reads three large files, writes a large one, then
reads another: about 80,000 tokens, past the small window's compaction threshold.

What it checks:
  large window: no compaction, every request asks for the full reply cap, none is refused;
  small window: the conversation is compacted, the request after it carries the person's request
                word for word, requests near the window ask for no more than the room left, and
                none is refused;
  --max-time:   a slow reply under a 3-second limit stops near the limit with stop_reason max_time.

Usage: scripts/check-window-scaling.py [--env NAME=VALUE ...] [path/to/kodact]
       (default: target/debug/kodact). `--env` sets a variable for kodact's runs, after the
       script's own: `--env KODACT_BACKGROUND_SUMMARY=1` checks a setting under test.
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REQUEST = (
    "Read big_1.txt, big_2.txt and big_3.txt, write notes.txt with what you found, read big_4.txt, "
    "then answer done. Request id: WINDOW-CHECK-7Q."
)
CAP = 32_000


def words(n_chars: int, seed: int) -> str:
    vocab = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet"]
    out, i = [], seed
    while sum(len(w) + 1 for w in out) < n_chars:
        out.append(vocab[i % len(vocab)] + str(i % 97))
        i = i * 7 + 3
    lines = [" ".join(out[k:k + 12]) for k in range(0, len(out), 12)]
    return "\n".join(lines) + "\n"


class Model:
    """The scripted model: one step per request that isn't a summary request."""

    def __init__(self, window: int, work: str, slow: bool = False):
        self.window, self.work, self.slow = window, work, slow
        self.step = 0
        self.log = []
        self.steps = [
            ("Read", {"file_path": f"{work}/big_1.txt"}),
            ("Read", {"file_path": f"{work}/big_2.txt"}),
            ("Read", {"file_path": f"{work}/big_3.txt"}),
            ("Write", {"file_path": f"{work}/notes.txt", "content": words(80_000, 5)}),
            ("Read", {"file_path": f"{work}/big_4.txt"}),
            None,
        ]


def handler_for(model: Model):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send_json(self, code, obj):
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            self.send_json(200, {"object": "list", "data": [{"id": "check-model", "object": "model"}]})

        def chunk(self, delta=None, finish=None, usage=None):
            c = {"id": "x", "object": "chat.completion.chunk", "created": int(time.time()), "model": "check-model",
                 "choices": [] if usage else [{"index": 0, "delta": delta or {}, "finish_reason": finish}]}
            if usage:
                c["usage"] = usage
            self.wfile.write(f"data: {json.dumps(c)}\n\n".encode())
            self.wfile.flush()

        def do_POST(self):
            req = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))
            prompt = (len(json.dumps(req.get("messages", []))) + len(json.dumps(req.get("tools", [])))) // 4
            asked = req.get("max_tokens") or req.get("max_completion_tokens") or 0
            users = [m for m in req["messages"] if m.get("role") == "user"]
            text = lambda m: m["content"] if isinstance(m.get("content"), str) else json.dumps(m.get("content"))
            summary = bool(users) and "replaced by a summary" in text(users[-1])
            first_user = text(users[0]) if users else ""
            entry = {"prompt": prompt, "asked": asked, "summary": summary,
                     "carries_request": "The person's requests" in first_user and REQUEST in first_user}
            model.log.append(entry)
            # The endpoint counts the requested reply against the window, as vLLM does.
            if prompt + asked > model.window:
                entry["refused"] = True
                msg = (f"This model's maximum context length is {model.window} tokens. However, you requested "
                       f"{asked} output tokens and your prompt contains at least {prompt} input tokens, for a total "
                       f"of at least {prompt + asked} tokens. Please reduce the length of the input prompt or the "
                       "number of requested output tokens.")
                return self.send_json(400, {"error": {"message": msg, "type": "BadRequestError", "code": 400}})
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.chunk({"role": "assistant"})
            if model.slow:
                for _ in range(20):
                    time.sleep(0.5)
                    self.chunk({"content": "."})
            if summary:
                reply, call = "<summary>Read the files; notes next.</summary>", None
            else:
                call = model.steps[min(model.step, len(model.steps) - 1)]
                model.step += 1
                reply = "done" if call is None else "Next step."
            self.chunk({"content": reply})
            if call:
                name, args = call
                self.chunk({"tool_calls": [{"index": 0, "id": f"call_{len(model.log)}", "type": "function",
                                            "function": {"name": name, "arguments": json.dumps(args)}}]})
            self.chunk(finish="tool_calls" if call else "stop")
            self.chunk(usage={"prompt_tokens": prompt, "completion_tokens": 50, "total_tokens": prompt + 50})
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()

    return H


def run(kodact: str, window: int, extra=(), slow=False):
    work = tempfile.mkdtemp(prefix="kodact-window-")
    for i in range(1, 5):
        with open(f"{work}/big_{i}.txt", "w") as f:
            f.write(words(46_000, i))
    model = Model(window, work, slow)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(model))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    home = tempfile.mkdtemp(prefix="kodact-home-")
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_")}
    env.update({
        "HOME": home,
        "KODACT_OPENAI_BASE_URL": f"http://127.0.0.1:{server.server_address[1]}/v1",
        "KODACT_OPENAI_API_KEY": "check",
        "KODACT_CONTEXT_WINDOW": str(window),
        "NO_PROXY": "127.0.0.1,localhost",
        "no_proxy": "127.0.0.1,localhost",
    })
    env.update(EXTRA_ENV)
    args = [kodact, "-p", REQUEST, "--output-format", "stream-json", "--verbose", "--model", "check-model",
            "--dangerously-skip-permissions", *extra]
    t0 = time.time()
    p = subprocess.run(args, cwd=work, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300)
    elapsed = time.time() - t0
    server.shutdown()
    events = [json.loads(line) for line in p.stdout.splitlines() if line.startswith("{")]
    compactions = sum(1 for e in events if e.get("type") == "system" and e.get("subtype") == "compact_boundary")
    result = next((e for e in events if e.get("type") == "result"), {})
    for e in events:
        if e.get("type") == "system" and str(e.get("subtype", "")).startswith("summary_ahead"):
            print(f"   [{e['subtype']}] {json.dumps({k: v for k, v in e.items() if k not in ('type', 'subtype', 'session_id', 'uuid')})}")
    return model.log, compactions, result, elapsed, p.stderr


def show(name, log, compactions, result):
    print(f"\n== {name}: {len(log)} requests, {compactions} compaction(s), result "
          f"{result.get('subtype')} / {result.get('stop_reason')}")
    print(f"   {'prompt':>8} {'asked':>7} {'room':>8}  note")
    for e in log:
        note = []
        if e["summary"]:
            note.append("summary call")
        if e["carries_request"]:
            note.append("carries the request word for word")
        if e.get("refused"):
            note.append("REFUSED")
        print(f"   {e['prompt']:>8} {e['asked']:>7} {e['window_room']:>8}  {', '.join(note)}")


EXTRA_ENV = {}


def main():
    argv = sys.argv[1:]
    while len(argv) >= 2 and argv[0] == "--env":
        name, _, value = argv[1].partition("=")
        EXTRA_ENV[name] = value
        argv = argv[2:]
    kodact = os.path.abspath(argv[0] if argv else "target/debug/kodact")
    failures = []

    for window, name in [(1_000_000, "large window (1,000,000)"), (100_000, "small window (100,000)")]:
        log, compactions, result, _, stderr = run(kodact, window)
        for e in log:
            e["window_room"] = window - e["prompt"]
        show(name, log, compactions, result)
        refused = [e for e in log if e.get("refused")]
        if refused:
            failures.append(f"{name}: {len(refused)} request(s) refused for length")
        if result.get("subtype") != "success":
            failures.append(f"{name}: the run ended {result.get('subtype')}: {stderr[-500:]}")
        if window >= 1_000_000:
            if compactions:
                failures.append(f"{name}: compacted at about {max(e['prompt'] for e in log)} tokens")
            if any(e["asked"] != CAP for e in log):
                failures.append(f"{name}: asked for less than the cap {CAP}: {[e['asked'] for e in log]}")
        else:
            if not compactions:
                failures.append(f"{name}: never compacted")
            after = [e for e in log if not e["summary"] and e["carries_request"]]
            if not after:
                failures.append(f"{name}: no request after compaction carried the person's request word for word")
            near = [e for e in log if e["prompt"] > window - CAP - window // 50]
            ahead = EXTRA_ENV.get("KODACT_BACKGROUND_SUMMARY", "0") not in ("", "0", "false", "off", "no")
            if not near and ahead:
                # The summary written ahead was the request that came near the window: nothing does.
                print(f"   note: with the summary written ahead, no request came near the window; the "
                      f"reply budget near the window is checked by the run without --env")
            elif not near:
                failures.append(f"{name}: no request came near the window, so the reply budget wasn't tested")
            elif any(e["asked"] >= CAP for e in near):
                failures.append(f"{name}: a request near the window still asked for the full cap")

    log, _, result, elapsed, _ = run(kodact, 1_000_000, extra=["--max-time", "3"], slow=True)
    print(f"\n== --max-time 3 with a slow reply: stopped after {elapsed:.1f}s, result "
          f"{result.get('subtype')} / {result.get('stop_reason')}")
    if result.get("stop_reason") != "max_time" or elapsed > 8:
        failures.append(f"--max-time: {result.get('subtype')} / {result.get('stop_reason')} after {elapsed:.1f}s")

    print()
    if failures:
        print("FAILED:\n- " + "\n- ".join(failures))
        sys.exit(1)
    print("ok: limits follow the model's window; the large window is left alone")


if __name__ == "__main__":
    main()
