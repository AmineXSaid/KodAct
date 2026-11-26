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

The same request, `ask()`, is how endpoint_gate.py tells whether the endpoint answers before a
trial starts: it costs no tokens, and it goes through the name lookup, TLS, the key, the gateway
and the server behind it.
"""

import http.client
import json
import os
import re
import ssl
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
    # vLLM's newer wording: "Input length (72614) exceeds model's maximum context length (65536)."
    re.compile(r"maximum context length \((\d+)\)"),
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


def ask(base: str, key: str | None, model: str, timeout: float = 30) -> tuple[int | None, str]:
    """Send the request for an impossible reply: (HTTP status, body), or (None, why) when nothing
    answered (a name that doesn't resolve, a refused connection, a TLS failure, a timeout).

    Trusts the system's certificates and KODACT_HARBOR_CA_BUNDLE, as the task containers do, and
    goes through this machine's proxy settings."""
    headers = {"Content-Type": "application/json", "User-Agent": "kodact-harbor"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
    body = {"model": model, "max_tokens": 100_000_000, "messages": [{"role": "user", "content": "hi"}]}
    req = urllib.request.Request(f"{base.rstrip('/')}/chat/completions", data=json.dumps(body).encode(), headers=headers)
    ctx = ssl.create_default_context()
    bundle = os.path.expanduser(os.environ.get("KODACT_HARBOR_CA_BUNDLE", ""))
    if bundle and os.path.isfile(bundle):
        ctx.load_verify_locations(cafile=bundle)
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
            return r.status, r.read(65536).decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        try:
            text = e.read(65536).decode("utf-8", "replace")
        except (OSError, http.client.HTTPException):
            text = ""  # it answered, with a status; the body was lost
        return e.code, text
    except urllib.error.URLError as e:
        return None, str(e.reason)
    except (OSError, ValueError, http.client.HTTPException) as e:
        return None, str(e) or type(e).__name__


def main() -> None:
    base = os.environ.get("KODACT_OPENAI_BASE_URL", "").rstrip("/")
    if not base or len(sys.argv) < 2:
        return
    model = sys.argv[1].split("/", 1)[-1]  # as the agent sends it: a provider prefix is dropped
    status, text = ask(base, os.environ.get("KODACT_OPENAI_API_KEY"), model)
    if status is None:
        return
    served, window = parse(text)
    print(f"{served}\t{window}")


if __name__ == "__main__":
    main()
