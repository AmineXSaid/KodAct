"""A stand-in OpenAI-compatible endpoint whose behaviour each test case sets.

The case is read from CONFIG (a JSON file) on every request, so the runner can switch
cases without restarting. Every request is appended to LOG as one JSON line.
"""
import json, sys, time, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
PORT, CONFIG, LOG = int(sys.argv[1]), sys.argv[2], sys.argv[3]
lock = threading.Lock()
def cfg():
    try: return json.load(open(CONFIG))
    except Exception: return {}
def log(rec):
    with lock, open(LOG, "a") as f: f.write(json.dumps(rec) + "\n")
class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def reply(self, status, body, ctype="application/json"):
        b = body if isinstance(body, bytes) else body.encode()
        self.send_response(status); self.send_header("content-type", ctype); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        c = cfg(); t0 = time.time()
        log({"t": t0, "method": "GET", "path": self.path})
        m = c.get("models", {})
        if m.get("hang"): time.sleep(m["hang"]); 
        time.sleep(m.get("delay", 0))
        if self.path.rstrip("/").endswith("/models"):
            if "raw" in m: return self.reply(m.get("status", 200), m["raw"], m.get("ctype", "application/json"))
            entry = {"id": c.get("model", "probe-model"), "object": "model", **m.get("fields", {})}
            data = m.get("extra", []) + ([] if m.get("omit_model") else [entry])
            return self.reply(m.get("status", 200), json.dumps({"object": "list", "data": data}))
        self.reply(404, '{"error":{"message":"not found"}}')
    def do_POST(self):
        c = cfg(); n = int(self.headers.get("content-length", 0)); req = json.loads(self.rfile.read(n) or b"{}")
        msgs = req.get("messages", [])
        summary = any("summary" in json.dumps(m).lower() and "<summary>" in json.dumps(m) for m in msgs[-1:]) or req.get("tool_choice") == "none"
        log({"t": time.time(), "method": "POST", "path": self.path, "max_tokens": req.get("max_tokens"), "max_completion_tokens": req.get("max_completion_tokens"),
             "n_messages": len(msgs), "tool_choice": req.get("tool_choice"), "summary": summary, "bytes": n})
        ch = c.get("chat", {})
        time.sleep(ch.get("delay", 0))
        if ch.get("error_over") and n > ch["error_over"]:
            return self.reply(400, json.dumps({"error": {"message": ch.get("error_text", "This model's maximum context length is 32768 tokens. However, your messages resulted in 40000 tokens."), "type": "invalid_request_error", "code": "context_length_exceeded"}}))
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.send_header("transfer-encoding", "chunked"); self.end_headers()
        def ev(d):
            b = b"data: " + json.dumps(d).encode() + b"\n\n"
            self.wfile.write(b"%x\r\n%s\r\n" % (len(b), b)); self.wfile.flush()
        base = {"id": "c", "object": "chat.completion.chunk", "model": c.get("model", "probe-model")}
        text = "<analysis>a</analysis><summary>Earlier: the user said hello.</summary>" if summary else ch.get("text", "ok")
        ev({**base, "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}]})
        # Reported prompt size: a fixed number per case, or grow with the conversation.
        pt = ch.get("prompt_tokens", 3000) + ch.get("per_message", 0) * len(msgs)
        ev({**base, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": pt, "completion_tokens": 5, "total_tokens": pt + 5}})
        b = b"data: [DONE]\n\n"; self.wfile.write(b"%x\r\n%s\r\n0\r\n\r\n" % (len(b), b)); self.wfile.flush()
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
