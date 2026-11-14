"""A scripted OpenAI chat-completions model for the `named-elsewhere` task.

It does what a model does with a one-function fix: it reads src/members.py, writes the fix, runs
the tests the request names and ends. Told that files are missing, it answers that the names
were only words and ends again: what is measured is whether KodAct says so at all (the
`constraints` reminder, the result's `verification.missing`), not what the model then makes.
"""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

FIXED = '''"""Read the rows of a members export: one `id,name` per line."""


def parse_rows(lines):
    """The (id, name) pairs of the rows, in order, with the id as an int.

    A row without an id is skipped, and a repeated id keeps its first row.
    """
    rows, seen = [], set()
    for line in lines:
        line = line.strip()
        if not line:
            continue
        id_text, name = line.split(",", 1)
        if not id_text.strip():
            continue
        row_id = int(id_text)
        if row_id in seen:
            continue
        seen.add(row_id)
        rows.append((row_id, name.strip()))
    return rows
'''


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

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        done = {m["tool_call_id"] for m in req["messages"] if m["role"] == "tool"}
        last = text_of(req["messages"][-1]) if req["messages"] else ""
        if "call_Bash" in done and "names files that don't exist" in last:
            out = say("Those names are words of the request, not files: members.py is src/members.py, and nothing is missing.")
        elif "call_Bash" in done:
            out = say("parse_rows in src/members.py now skips rows without an id and keeps the first row of a repeated id; the tests pass.")
        elif "call_Write" in done:
            out = call("Bash", {"command": "python3 -m unittest discover -s tests", "description": "Run the tests"})
        elif "call_Read" in done:
            out = call("Write", {"file_path": "src/members.py", "content": FIXED})
        else:
            out = call("Read", {"file_path": "src/members.py"})
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
