"""A scripted OpenAI chat-completions model for the `think-after-two-stops` task.

It does what quick-thinking did on write-compressor and regex-chess (d3787e1, 2026-11-23): its
replies only reason, slowly, until something stops them; stopped three times, it acts. Each reply
streams reasoning for 90 s and then ends at its cap (finish_reason "length", no text), as a reply
of 32,000 tokens did there in about 4 minutes. KodAct's thinking time-box stops a reply at its
limit (30 s under `--max-time 120`): it used to stop two in a row and let the third run to its cap,
which here takes the run past its time limit before the model acts. So what is measured is how
KodAct bounds thinking that leads nowhere, not the model.

    python3 evals/scripted/openai_slow_thinker.py 8801
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

STOPS_BEFORE_ACTING = 3
REPLY_SECS = 90
# What KodAct says when it stops a reply: the time-box's handback, or the cut-off's reminder.
STOPPED = ("this step's time is up", "reached the output token limit")


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"


def call(call_id, name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")


def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")


def text_of(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""


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
        done = {m.get("tool_call_id") for m in msgs if m.get("role") == "tool"}
        # Counted in the text, not by message: KodAct sends consecutive user messages as one.
        stops = sum(text_of(m).count(s) for m in msgs if m.get("role") == "user" for s in STOPPED)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        try:
            if "call_write" in done:
                self.send(say("answer.txt holds the answer.") + "data: [DONE]\n\n")
            elif stops >= STOPS_BEFORE_ACTING:
                self.send(call("call_write", "Write", {"file_path": "answer.txt", "content": "42\n"}) + "data: [DONE]\n\n")
            else:
                # Reasoning, a little at a time, until KodAct stops the reply or the cap ends it.
                self.send(chunk({"role": "assistant"}))
                start = time.monotonic()
                while time.monotonic() - start < REPLY_SECS:
                    self.send(chunk({"reasoning_content": "the next bit of the encoding depends on the one before, so "}))
                    time.sleep(0.5)
                self.send(chunk({}, "length") + "data: [DONE]\n\n")
        except (BrokenPipeError, ConnectionResetError):
            pass  # stopped: KodAct closed the stream


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
