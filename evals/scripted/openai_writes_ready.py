"""A scripted OpenAI chat-completions model for the `dash-prompt` task.

It writes `ready` into ready.txt and ends. The task's instruction starts with "- ", as a pasted
list does: what is measured is whether kodact starts at all with such a prompt, not the model.
"""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


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
        out = say("done") if "call_Write" in done else call("Write", {"file_path": "ready.txt", "content": "ready"})
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
