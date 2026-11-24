"""A scripted OpenAI chat-completions model for the `summary-thinks-long` task.

It does what quick-thinking did on regex-chess (d3787e1, 2026-11-23): asked for a summary, it
reasons, slowly, and writes nothing, the first time and again when its own analysis comes back:
there, three summaries reasoned to their cap (22,000 to 30,000 tokens, about 4 minutes each).
Here a summary streams reasoning for 90 s, then ends at its cap (finish_reason "length", no text).
The conversation outgrows the window once (the second work request is refused with the
vLLM/LiteLLM wording), so KodAct must compact, and after two empty summaries it writes its own
note. With `nudges.thinkingTimeBox` an automatic summary is stopped at the time-box's limit (30 to
40 s under `--max-time 160`); without it both run to their cap and the run's time is up first. So
what is measured is how long KodAct's compaction waits on summaries that lead nowhere, not the
model.

    python3 evals/scripted/openai_slow_summary.py 8802
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SUMMARY_ASK = "The conversation is about to be replaced by a summary"
REPLY_SECS = 90
OVERFLOW = ("This model's maximum context length is 100000 tokens. However, your messages resulted in "
            "100500 tokens. Please reduce the length of the messages.")


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"


def call(name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": f"call_{name}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")


def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")


def text_of(message):
    c = message.get("content") or ""
    return c if isinstance(c, str) else " ".join(p.get("text", "") for p in c if isinstance(p, dict))


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        b = json.dumps({"data": [{"id": "m"}]}).encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b)

    def send(self, text):
        self.wfile.write(text.encode())
        self.wfile.flush()

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        msgs = req["messages"]
        last = text_of(msgs[-1]) if msgs else ""
        done = {m.get("tool_call_id") for m in msgs if m.get("role") == "tool"}
        # After a compaction the conversation starts from KodAct's summary or its own note.
        compacted = any(
            m.get("role") == "user" and ("continues an earlier conversation" in text_of(m) or "could not be summarized" in text_of(m))
            for m in msgs
        )
        if SUMMARY_ASK not in last and "call_Bash" in done and not compacted and "call_Write" not in done:
            # The conversation no longer fits: refused, as vLLM behind LiteLLM refuses it.
            b = json.dumps({"error": {"message": OVERFLOW, "type": "invalid_request_error"}}).encode()
            self.send_response(400)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(b)
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        try:
            if SUMMARY_ASK in last:
                self.send(chunk({"role": "assistant"}))
                start = time.monotonic()
                while time.monotonic() - start < REPLY_SECS:
                    self.send(chunk({"reasoning_content": "the person asked for the line count, and wc said 1000, so "}))
                    time.sleep(0.5)
                self.send(chunk({}, "length") + "data: [DONE]\n\n")
            elif "call_Write" in done:
                self.send(say("done.txt holds 1000.") + "data: [DONE]\n\n")
            elif compacted:
                self.send(call("Write", {"file_path": "done.txt", "content": "1000"}) + "data: [DONE]\n\n")
            else:
                self.send(call("Bash", {"command": "wc -l data.txt", "description": "Count the lines"}) + "data: [DONE]\n\n")
        except (BrokenPipeError, ConnectionResetError):
            pass  # stopped: KodAct closed the stream


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
