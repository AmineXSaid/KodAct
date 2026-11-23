"""A scripted OpenAI chat-completions model for the `verify-process-lookup` task.

It does what quick-thinking did on kv-store-grpc, 2026-11-23: it fixes the file, runs it (a check
that passes), then looks for a running copy, and ends. KodAct took that lookup for a failed check
of the file and reminded the model after its last reply; told so, this model answers once more.
So what is measured is KodAct's verification loop (`verify_reminders`, turns), not the model.

The lookup is `pgrep -x greet.py`, which finds no process of that name (exit 1). With `missing`
after the port it is `procwatch greet.py`, a lookup tool that isn't installed (exit 127), as
pgrep wasn't in that container. (`pgrep -f "python3 greet.py"` would match its own shell's command
line and pass, so it shows nothing either way.)
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MISSING = sys.argv[2:] == ["missing"]
LOOKUP = "procwatch greet.py" if MISSING else "pgrep -x greet.py"


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


def reply(messages):
    done = {m.get("tool_call_id") for m in messages if m.get("role") == "tool"}
    steps = [
        ("call_read", "Read", {"file_path": "greet.py"}),
        ("call_write", "Write", {"file_path": "greet.py", "content": 'print("hello, world")\n'}),
        ("call_run", "Bash", {"command": "python3 greet.py", "description": "Run the fixed script"}),
        ("call_lookup", "Bash", {"command": LOOKUP, "description": "Look for a running copy"}),
    ]
    for call_id, name, args in steps:
        if call_id not in done:
            return call(call_id, name, args)
    # KodAct's reminder after the last reply: answered once, as a model does.
    last = text_of(messages[-1]) if messages else ""
    if messages[-1].get("role") == "user" and "<system-reminder>" in last:
        return say("The lookup found no running copy, which is what was asked; greet.py was checked by running it.")
    return say("greet.py now prints `hello, world` (checked by running it), and no copy of it is running.")


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        b = json.dumps({"data": [{"id": "m"}]}).encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b)

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        out = reply(req["messages"])
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
