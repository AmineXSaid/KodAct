"""A scripted OpenAI chat-completions model for the `judge-reasons-long` task.

The agent does what the deep-thinking false successes of 2026-11-16 and 17 did: it fixes one of
two bugs, runs the visible test (which covers only that one), and ends with Finish claiming both
fixed and `verified`. Told by KodAct's separate check that the request isn't met, it fixes the
second bug, runs a check that shows it, and calls Finish again.

The same endpoint answers the separate check (a request with no tools whose system prompt is
the checker's) as a reasoning model does: it reasons for REASONING_TOKENS before it writes its
verdict, from the same budget as the verdict, as DeepSeek's thinking mode does. A request whose
reply cap is smaller is cut off while it reasons (finish_reason "length", no text), and the
check judges nothing. Its verdict reads the transcript only: "not met" while no tool result shows
median() of an even-length list working, "met" once one does.

So what is measured is whether KodAct gives its check the room to answer, not what the model
knows. A word stands for a token.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REASONING_TOKENS = 10_000
CHECKER = "independent checker"
NOTE = "A separate check"

FIRST = """def mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


def median(xs):
    s = sorted(xs)
    return s[len(s) // 2]
"""

SECOND = """def mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


def median(xs):
    s = sorted(xs)
    n = len(s)
    if n % 2:
        return s[n // 2]
    return (s[n // 2 - 1] + s[n // 2]) / 2
"""

TEST = "python3 test_stats.py"
MEDIAN = 'python3 test_stats.py && python3 -c "from stats import median; print(median([1, 2, 3, 4]))"'

FIRST_FINISH = {
    "summary": "Fixed both bugs in stats.py; python3 test_stats.py passes.",
    "requirements": [
        {"requirement": "mean([]) returns 0.0", "scope": "the empty list", "evidence": f"`{TEST}` printed test_stats: mean ok", "met": True},
        {"requirement": "median() of an even-length list averages the two middle values", "scope": "even-length lists", "evidence": f"`{TEST}` passed", "met": True},
    ],
    "verified": True,
}

SECOND_FINISH = {
    "summary": "Fixed both bugs in stats.py: mean([]) is 0.0 and median([1, 2, 3, 4]) is 2.5.",
    "requirements": [
        {"requirement": "mean([]) returns 0.0", "scope": "the empty list", "evidence": f"`{TEST}` printed test_stats: mean ok", "met": True},
        {"requirement": "median() of an even-length list averages the two middle values", "scope": "median([1, 2, 3, 4])", "evidence": f"`{MEDIAN}` printed 2.5", "met": True},
    ],
    "verified": True,
}


def chunk(delta, finish=None, usage=None):
    body = {"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
    if usage:
        body["usage"] = usage
    return "data: " + json.dumps(body) + "\n\n"


def call(call_id, name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")


def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")


def text_of(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""


def reasoning_words(n):
    """n words of a checker weighing the evidence, numbered so no passage repeats."""
    words = []
    step = 0
    while len(words) < n:
        step += 1
        words += f"Step {step}: weigh each requirement against the tool results, not the agent's words.".split()
    return words[:n]


def check(req):
    cap = req.get("max_completion_tokens") or req.get("max_tokens") or 0
    transcript = "\n\n".join(text_of(m) for m in req["messages"] if m.get("role") == "user")
    # The checker's transcript puts each entry in its own paragraph: a Bash result that printed
    # 2.5 is the evidence; the request's own "is 2.5" and the agent's claims are not.
    shown = any(b.startswith("TOOL RESULT (Bash):") and "2.5" in b for b in transcript.split("\n\n"))
    if shown:
        verdict = {"verdict": "met", "evidence": ["median([1, 2, 3, 4]) printed 2.5"], "reason": "both fixes are shown working"}
    else:
        verdict = {"verdict": "not_met", "evidence": [], "reason": "no tool result shows median() of an even-length list: test_stats.py covers mean() only"}
    answer = json.dumps(verdict)
    needed = REASONING_TOKENS + len(answer.split())
    out = chunk({"role": "assistant"})
    words = reasoning_words(min(cap, REASONING_TOKENS))
    for i in range(0, len(words), 200):
        out += chunk({"reasoning_content": " ".join(words[i:i + 200]) + " "})
    if cap < needed:
        return out + chunk({}, "length", {"prompt_tokens": 1, "completion_tokens": cap, "total_tokens": cap + 1})
    out += chunk({"content": answer})
    return out + chunk({}, "stop", {"prompt_tokens": 1, "completion_tokens": needed, "total_tokens": needed + 1})


def work(messages):
    done = {m.get("tool_call_id") for m in messages if m.get("role") == "tool"}
    told = any(m.get("role") == "user" and NOTE in text_of(m) for m in messages)
    steps = [
        ("call_read", "Read", {"file_path": "stats.py"}),
        ("call_write1", "Write", {"file_path": "stats.py", "content": FIRST}),
        ("call_test1", "Bash", {"command": TEST}),
        ("call_finish1", "Finish", FIRST_FINISH),
    ]
    if told:
        steps += [
            ("call_write2", "Write", {"file_path": "stats.py", "content": SECOND}),
            ("call_test2", "Bash", {"command": MEDIAN}),
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
        system = " ".join(text_of(m) for m in req["messages"] if m.get("role") == "system")
        out = check(req) if not req.get("tools") and CHECKER in system else work(req["messages"])
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
