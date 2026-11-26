"""A stand-in OpenAI-compatible endpoint whose behaviour each test case sets.

The case is read from CONFIG (a JSON file) on every request, so the runner can switch
cases without restarting. Every request is appended to LOG as one JSON line.
"""
import json, sys, time, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
PORT, CONFIG, LOG = int(sys.argv[1]), sys.argv[2], sys.argv[3]
lock = threading.Lock()
seen = {}
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
        # LiteLLM's per-model limits page, when the case gives one.
        if self.path.rstrip("/").endswith("/model/info") and "model_info" in m:
            data = [{"model_name": c.get("model", "probe-model"), "litellm_params": {"model": "hosted_vllm/x"}, "model_info": m["model_info"]}]
            return self.reply(200, json.dumps({"data": data}))
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
        # A server with a real window that refuses, in vLLM's words passed on by LiteLLM,
        # a prompt plus requested reply larger than it. `real_extra`: the prompt it counts is that
        # much larger than the size its replies report (a tokenizer that counts denser text than
        # KodAct estimates). `at_least`: like vLLM, it states only a lower bound for the prompt,
        # the window less the reply asked for, plus one.
        pt = ch.get("prompt_tokens", 3000) + ch.get("per_message", 0) * len(msgs) + ch.get("real_extra", 0)
        # `fill_first`: the first reply's prompt fills the window to the token, later ones don't.
        if ch.get("fill_first") and not seen.get("filled"):
            seen["filled"] = True; pt = ch["window"]
        mt = req.get("max_tokens") or req.get("max_completion_tokens") or 0
        # A server that caps replies below what was asked, refusing in the words of the API it
        # stands for (`max_output_style`): OpenAI's (default) or DeepSeek's.
        if ch.get("max_output") and mt > ch["max_output"]:
            words = {
                "openai": f"max_tokens is too large: {mt}. This model supports at most {ch['max_output']} completion tokens, whereas you provided {mt}.",
                "deepseek": f"Invalid max_tokens value, the valid range of max_tokens is [1, {ch['max_output']}]",
            }[ch.get("max_output_style", "openai")]
            return self.reply(400, json.dumps({"error": {"message": words, "type": "invalid_request_error"}}))
        # A server that clamps the reply asked for to the window's room (a newer vLLM), and says so
        # only when no room is left: a prompt over the window is refused as "Input length (N)
        # exceeds model's maximum context length (W).", one that fills it to the token with a
        # reply budget of nothing ("max_tokens must be at least 1, got 0").
        if ch.get("clamp") and ch.get("window"):
            if pt > ch["window"]:
                inner = f"Input length ({pt}) exceeds model's maximum context length ({ch['window']})."
                return self.reply(400, json.dumps({"detail": "litellm.BadRequestError: Hosted_vllmException - " + json.dumps({"error": {"message": inner, "type": "BadRequestError", "param": None, "code": 400}})}))
            if pt == ch["window"]:
                inner = "max_tokens must be at least 1, got 0. (parameter=max_tokens, value=0)"
                return self.reply(400, json.dumps({"detail": "litellm.BadRequestError: Hosted_vllmException - " + json.dumps({"error": {"message": inner, "type": "BadRequestError", "param": "max_tokens", "code": 400}})}))
        elif ch.get("window") and pt + mt > ch["window"]:
            stated = ch["window"] - mt + 1 if ch.get("at_least") else pt
            inner = (f"This model's maximum context length is {ch['window']} tokens. However, you requested {mt} output tokens "
                     f"and your prompt contains at least {stated} input tokens, for a total of at least {stated + mt} tokens.")
            return self.reply(400, json.dumps({"detail": "litellm.ContextWindowExceededError: litellm.BadRequestError: " + json.dumps({"error": {"message": inner}})}))
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
