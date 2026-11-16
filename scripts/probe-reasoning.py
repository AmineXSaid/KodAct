#!/usr/bin/env python3
"""Does an OpenAI-compatible endpoint pass a model's reasoning back to it?

KodAct's `carryReasoning` sends the reasoning of the replies in a conversation back with each
request, as `reasoning_content` (DeepSeek's thinking mode asks for it when tools are used). An
endpoint can refuse the field, require it, accept it and drop it, or pass it to the model. This
tells them apart with a small tool loop, sent the way KodAct sends one: the tool's result is the
last message, with no user message after it.

1. The model is asked to call a tool, then to reply with the secret number noted in its
   reasoning before the call.
2. The probe sends the tool's result back twice. Once with reasoning that notes a random
   eight-digit number of its own choosing, once with no reasoning.

Passed to the model, the reply gives that number. Dropped, the model can't know it (one chance in
a hundred million to guess it).

Needs no package. It reads the endpoint the way kodact does:

  KODACT_OPENAI_BASE_URL   the endpoint, ending in /v1 (as for kodact)
  KODACT_OPENAI_API_KEY    its key (never printed)

Usage: scripts/probe-reasoning.py MODEL [--repeat N] [--max-tokens N] [--timeout S]
"""

import argparse
import json
import os
import random
import re
import ssl
import sys
import time
import urllib.error
import urllib.request

ASK = (
    "Call the get_number tool. When it has answered, reply with the secret number noted in your "
    "reasoning before the call, and nothing else. If you have no such number, reply NONE."
)
TOOLS = [{
    "type": "function",
    "function": {
        "name": "get_number",
        "description": "Returns a number.",
        "parameters": {"type": "object", "properties": {}},
    },
}]
EIGHT = re.compile(r"(?<!\d)\d{8}(?!\d)")
# Separators inside a written number: 12,345,678 or 1234 5678.
SEPARATOR = re.compile(r"(?<=\d)[ ,_'  ](?=\d)")


def post(url, key, body, timeout):
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {key}"},
        method="POST",
    )
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE") or None)
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
            return r.status, json.loads(r.read()), time.time() - t0
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace"), time.time() - t0
    except Exception as e:  # network, timeout, a body that isn't JSON
        return None, f"{type(e).__name__}: {e}", time.time() - t0


def message(resp):
    return ((resp.get("choices") or [{}])[0]).get("message") or {}


def reasoning_of(msg):
    r = msg.get("reasoning_content") or msg.get("reasoning") or ""
    return r if isinstance(r, str) else json.dumps(r)


def numbers(text):
    """The eight-digit numbers in a text."""
    return set(EIGHT.findall(SEPARATOR.sub("", text or "")))


def short(resp):
    return (resp if isinstance(resp, str) else json.dumps(resp))[:300]


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("model")
    p.add_argument("--repeat", type=int, default=3, help="tool loops to try (default 3)")
    p.add_argument("--max-tokens", type=int, default=8000)
    p.add_argument("--timeout", type=int, default=300, help="seconds per request (default 300)")
    a = p.parse_args()

    base = os.environ.get("KODACT_OPENAI_BASE_URL", "").rstrip("/")
    key = os.environ.get("KODACT_OPENAI_API_KEY", "")
    if not base or not key:
        sys.exit("Set KODACT_OPENAI_BASE_URL and KODACT_OPENAI_API_KEY as for kodact.")
    url = base + "/chat/completions"

    def ask(messages):
        body = {"model": a.model, "messages": messages, "tools": TOOLS, "max_tokens": a.max_tokens}
        return post(url, key, body, a.timeout)

    rng = random.SystemRandom()
    tally = {"with": [0, 0], "without": [0, 0]}  # [the number came back, replies judged]
    refused = required = None
    reasoned = called = 0
    for n in range(1, a.repeat + 1):
        first = [{"role": "user", "content": ASK}]
        status, resp, secs = ask(first)
        if status != 200:
            print(f"loop {n}: the first request got HTTP {status} after {secs:.1f}s: {short(resp)}")
            continue
        msg = message(resp)
        calls = msg.get("tool_calls") or []
        reasoned += bool(reasoning_of(msg))
        if not calls:
            print(f"loop {n}: the model answered without calling the tool; not counted")
            continue
        called += 1
        secret = str(rng.randint(10**7, 10**8 - 1))
        noted = (f"The user wants the secret number once the tool has answered. The secret number is "
                 f"{secret}. First I call get_number.")
        # Every call answered, as the API requires; the result is the last message.
        results = [
            {"role": "tool", "tool_call_id": c.get("id"), "name": (c.get("function") or {}).get("name"),
             "content": "7"}
            for c in calls
        ]
        got = {}
        for mode in ("with", "without"):
            assistant = {"role": "assistant", "content": msg.get("content"), "tool_calls": calls}
            if mode == "with":
                assistant["reasoning_content"] = noted
            got[mode] = ask(first + [assistant] + results)
        (ws, wr, _), (os_, orr, _) = got["with"], got["without"]
        if ws in (400, 422) and os_ == 200:
            refused = short(wr)
        if os_ in (400, 422) and ws == 200:
            required = short(orr)
        for mode, (status, resp, secs) in got.items():
            if status != 200:
                print(f"loop {n} {mode:7}: HTTP {status} after {secs:.1f}s: {short(resp)}")
                continue
            reply = message(resp)
            answer = (reply.get("content") or "").strip()
            if not answer and reply.get("tool_calls"):
                print(f"loop {n} {mode:7}: the model called the tool again instead of answering; not counted")
                continue
            hit = secret in numbers(answer)
            tally[mode][0] += hit
            tally[mode][1] += 1
            print(f"loop {n} {mode:7}: answered {answer[:40]!r}; the secret {secret}: "
                  f"{'yes' if hit else 'no'} ({secs:.1f}s)")

    print()
    if called and not reasoned:
        print("The model sent no reasoning (reasoning_content) in its replies here. If it never does, "
              "carryReasoning has nothing to send back.")
    if refused:
        print(f"Refused: the request with the reasoning failed and the same one without it went through "
              f"({refused}). Leave carryReasoning off.")
        return
    if required:
        print(f"Required: the request without the reasoning failed and the same one with it went through "
              f"({required}). Turn carryReasoning on.")
        return
    w, wn = tally["with"]
    o, on = tally["without"]
    print(f"with the reasoning sent back: the secret came back {w} of {wn}; without: {o} of {on}")
    if not wn:
        print("Nothing could be judged: see the loops above.")
    elif w > o and 2 * w > wn:
        print("Passed to the model: carryReasoning gives it its reasoning back. Worth an A/B.")
    elif w == 0:
        print("Accepted, but the model doesn't get it (the endpoint drops the field): carryReasoning would "
              "cost tokens for nothing here.")
    else:
        print("Unclear: run it again with a higher --repeat.")


if __name__ == "__main__":
    main()
