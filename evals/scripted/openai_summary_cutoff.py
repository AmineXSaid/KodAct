"""A scripted OpenAI chat-completions model for the `summary-cutoff` task.

It does what quick-thinking did on 2026-11-17 (a 100k window): asked to summarize the whole
conversation, it reasons until the output limit stops it (finish_reason "length") and writes no
summary, every time. Given its own analysis back instead of the conversation, it writes the
summary. The conversation outgrows the window once, as a long run's does: the server refuses
the second work request with the vLLM/LiteLLM wording, and KodAct compacts. So what is measured
is how KodAct's compaction copes with a summary cut off while reasoning (compactions done,
failed, and KodAct's own note in their place), not the model.
"""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SUMMARY_ASK = "The conversation is about to be replaced by a summary"
ANALYSIS = "Looking at the conversation: the person asked for the number of lines of data.txt in done.txt. " * 40
OVERFLOW = ("This model's maximum context length is 100000 tokens. However, your messages resulted in "
            "100500 tokens. Please reduce the length of the messages.")
state = {"overflowed": False}


def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"


def call(name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": f"call_{name}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")


def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")


def reason_until_cut():
    return chunk({"role": "assistant"}) + chunk({"reasoning_content": ANALYSIS}) + chunk({}, "length")


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

    def reply(self, status, body, stream=True):
        self.send_response(status)
        self.send_header("content-type", "text/event-stream" if stream else "application/json")
        self.end_headers()
        self.wfile.write(body.encode())

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        msgs = req["messages"]
        last = text_of(msgs[-1]) if msgs else ""
        # A new run: no reply yet, and the task itself first (after a compaction a summary is).
        users = [text_of(m) for m in msgs if m["role"] == "user"]
        if not any(m["role"] == "assistant" for m in msgs) and users and users[0].startswith("Count the lines"):
            state["overflowed"] = False
        done = {m["tool_call_id"] for m in msgs if m["role"] == "tool"}
        if SUMMARY_ASK in last:
            out = say("<summary>Asked: the line count of data.txt into done.txt. `wc -l data.txt` said 1000. "
                      "Next: write 1000 to done.txt.</summary>") if "<earlier_analysis>" in last else reason_until_cut()
        elif "call_Write" in done:
            out = say("done.txt holds 1000.")
        elif state["overflowed"]:
            out = call("Write", {"file_path": "done.txt", "content": "1000"})
        elif "call_Bash" in done:
            state["overflowed"] = True
            return self.reply(400, json.dumps({"error": {"message": OVERFLOW, "type": "invalid_request_error"}}), stream=False)
        else:
            out = call("Bash", {"command": "wc -l data.txt", "description": "Count the lines"})
        self.reply(200, out + "data: [DONE]\n\n")


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
