#!/usr/bin/env python3
"""Which model an endpoint serves behind a name, and its context window, asked of the server.

    python3 evals/harbor/probe_model.py <model>      # prints "<served>\t<window>"; either may be empty

An alias can change behind your back: on 2026-11-11 one moved from Qwen 3.8 27B to DeepSeek V4
Flash between two days of runs, and their scores were compared as if from one model. This sends
one request that asks for an impossible reply (100,000,000 tokens). vLLM refuses it at once,
before generating anything, and the refusal names what answered: LiteLLM's "Received Model
Group=deepseek-v4-flash", vLLM's "max_model_len=524288". A server that answers instead gives its
reply's own `model` field.

Reads KODACT_OPENAI_BASE_URL and KODACT_OPENAI_API_KEY; the key goes in a header, never on a
command line. Information only: an endpoint that isn't OpenAI-compatible, or doesn't answer,
prints nothing and the run goes on.
"""

import json
import os
import re
import sys
import urllib.error
import urllib.request

SERVED = [
    re.compile(r"Received Model Group=([A-Za-z0-9._:/@-]+)"),
    re.compile(r"\bmodel=([A-Za-z0-9._:/@-]+)"),
]
WINDOW = [
    re.compile(r"max_model_len=(\d+)"),
    re.compile(r"maximum context length is (\d+)"),
    re.compile(r"maximum model length of (\d+)"),
]


def parse(body: str) -> tuple[str, str]:
    """The model a reply or refusal names, and the window it states ("" when it doesn't)."""
    served = next((m.group(1).rstrip(".") for p in SERVED if (m := p.search(body))), "")
    if not served:
        try:
            served = str(json.loads(body).get("model") or "")
        except (ValueError, AttributeError):
            served = ""
    window = next((m.group(1) for p in WINDOW if (m := p.search(body))), "")
    return served, window


def main() -> None:
    base = os.environ.get("KODACT_OPENAI_BASE_URL", "").rstrip("/")
    if not base or len(sys.argv) < 2:
        return
    model = sys.argv[1].split("/", 1)[-1]  # as the agent sends it: a provider prefix is dropped
    headers = {"Content-Type": "application/json"}
    if key := os.environ.get("KODACT_OPENAI_API_KEY"):
        headers["Authorization"] = f"Bearer {key}"
    body = {"model": model, "max_tokens": 100_000_000, "messages": [{"role": "user", "content": "hi"}]}
    req = urllib.request.Request(f"{base}/chat/completions", data=json.dumps(body).encode(), headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            text = r.read(65536).decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        text = e.read(65536).decode("utf-8", "replace")
    except (OSError, ValueError):
        return
    served, window = parse(text)
    print(f"{served}\t{window}")


if __name__ == "__main__":
    main()
