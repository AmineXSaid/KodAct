"""A scripted OpenAI chat-completions model for the `stopped-install` task.

It does what a model told to stop an install does: it starts the install in the background,
looks at its output once, stops it with KillShell, writes status.txt and ends. It never repairs
anything, so the eval passes only when the stop left the package database consistent: KodAct's
stop decides it, not the model's care. Each reply takes a few seconds, as a real model's does, so
the stop lands while a package is being set up.
"""
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

THINK_SECS = 3


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"


def call(name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": f"call_{name}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")


def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")


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
        time.sleep(THINK_SECS)
        if "call_Write" in done:
            out = say("I stopped the install and wrote `skipped` into status.txt.")
        elif "call_KillShell" in done:
            out = call("Write", {"file_path": "status.txt", "content": "skipped\n"})
        elif "call_BashOutput" in done:
            out = call("KillShell", {"shell_id": "bash_1"})
        elif "call_Bash" in done:
            out = call("BashOutput", {"bash_id": "bash_1"})
        else:
            out = call("Bash", {"command": "./tools/apt-get install fastcc fastlib fastld", "run_in_background": True, "description": "Install the toolchain"})
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
