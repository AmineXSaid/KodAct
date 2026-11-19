"""A scripted OpenAI chat-completions model for the `finish-unmet` task.

It does what 6 deep-thinking runs of 2026-11-16 and 17 did: it meets one requirement, checks it,
and ends with Finish saying another isn't met. Here the first report.py writes `count` and leaves
`total` out (the amounts are written as money, "$1,000.00"), and Finish lists `total` as not met,
with most of the run's time left. Told which requirements its Finish doesn't show met, it parses
the amounts, runs report.py again and ends with every requirement met. So what is measured is
whether KodAct answers such a Finish, not what the model knows.

With `met` after the port, it writes the full report.py first and its first Finish lists every
requirement met: the run KodAct should leave alone, for what the answer costs when it isn't due.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

FIRST = """import csv
import json

with open("orders.csv", newline="") as f:
    rows = list(csv.DictReader(f))
# The amounts are written as money ("$1,000.00"), not plain numbers: no total yet.
with open("report.json", "w") as f:
    json.dump({"count": len(rows)}, f)
"""

SECOND = """import csv
import json
from decimal import Decimal

with open("orders.csv", newline="") as f:
    rows = list(csv.DictReader(f))
cents = sum(int(Decimal(r["amount"].replace("$", "").replace(",", "")) * 100) for r in rows)
with open("report.json", "w") as f:
    json.dump({"count": len(rows), "total": cents}, f)
"""

RUN = "python3 report.py && cat report.json"

FIRST_FINISH = {
    "summary": "report.py writes report.json with `count` (3). `total` is not written: the amounts are formatted as money (\"$1,000.00\"), not plain numbers.",
    "requirements": [
        {"requirement": "report.json with `count`, the number of orders", "evidence": f"`{RUN}` printed {{\"count\": 3}}", "met": True},
        {"requirement": "`total`, the sum of their amounts in cents", "evidence": "report.json has no total: the amounts are money strings", "met": False},
    ],
    "verified": False,
}

SECOND_FINISH = {
    "summary": "report.py writes report.json with `count` 3 and `total` 101975 (the amounts parsed as money, in cents).",
    "requirements": [
        {"requirement": "report.json with `count`, the number of orders", "evidence": f"`{RUN}` printed \"count\": 3", "met": True},
        {"requirement": "`total`, the sum of their amounts in cents", "evidence": f"`{RUN}` printed \"total\": 101975 (1250 + 100000 + 725)", "met": True},
    ],
    "verified": True,
}

# KodAct's answer to a Finish that lists requirements it doesn't show met.
NOTE = "requirements not shown met"


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


MET = sys.argv[2:] == ["met"]


def reply(messages):
    done = {m.get("tool_call_id") for m in messages if m.get("role") == "tool"}
    told = any(m.get("role") == "user" and NOTE in text_of(m) for m in messages)
    steps = [
        ("call_write1", "Write", {"file_path": "report.py", "content": SECOND if MET else FIRST}),
        ("call_run1", "Bash", {"command": RUN}),
        ("call_finish1", "Finish", SECOND_FINISH if MET else FIRST_FINISH),
    ]
    if told:
        steps += [
            ("call_write2", "Write", {"file_path": "report.py", "content": SECOND}),
            ("call_run2", "Bash", {"command": RUN}),
            ("call_finish2", "Finish", SECOND_FINISH),
        ]
    for call_id, name, args in steps:
        if call_id not in done:
            return call(call_id, name, args)
    return say("done")


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
