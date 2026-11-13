#!/usr/bin/env python3
"""Does an OpenAI-compatible endpoint honour a reasoning level, and in which form?

KodAct's `--effort` reaches an endpoint as `reasoning_effort`. Some gateways ignore it, some
refuse it, some read another form. This sends one short reasoning question several times: with
no level, then each level in the two common forms (`reasoning_effort`, and `reasoning:
{"effort": ...}`), and prints what came back. A level that is honoured changes the reasoning
tokens; one that is ignored leaves them where the no-level request put them.

Needs no package and no API key in the command line: it reads the endpoint the way kodact does.

  KODACT_OPENAI_BASE_URL   the endpoint, ending in /v1 (as for kodact)
  KODACT_OPENAI_API_KEY    its key (never printed)

Usage: scripts/probe-effort.py MODEL [--repeat N] [--levels low,medium,high] [--max-tokens N]
"""

import argparse
import json
import os
import ssl
import statistics
import sys
import time
import urllib.error
import urllib.request

QUESTION = (
    "A list holds the integers 1 to 2000. Remove every number divisible by 3, then every number "
    "whose digits sum to an even number. How many numbers are left? Reply with the number only."
)


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
    except Exception as e:  # network, timeout
        return None, f"{type(e).__name__}: {e}", time.time() - t0


def facts(resp):
    """Reasoning tokens, output tokens, reasoning characters and the answer."""
    usage = resp.get("usage") or {}
    details = usage.get("completion_tokens_details") or {}
    msg = ((resp.get("choices") or [{}])[0]).get("message") or {}
    reasoning_text = msg.get("reasoning_content") or msg.get("reasoning") or ""
    if isinstance(reasoning_text, dict):
        reasoning_text = json.dumps(reasoning_text)
    return {
        "reasoning_tokens": details.get("reasoning_tokens"),
        "output_tokens": usage.get("completion_tokens"),
        "reasoning_chars": len(reasoning_text),
        "answer": (msg.get("content") or "").strip()[:40],
    }


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("model")
    p.add_argument("--repeat", type=int, default=2, help="requests per setting (default 2)")
    p.add_argument("--levels", default="low,medium,high")
    p.add_argument("--max-tokens", type=int, default=16000)
    p.add_argument("--timeout", type=int, default=300)
    a = p.parse_args()

    base = os.environ.get("KODACT_OPENAI_BASE_URL", "").rstrip("/")
    key = os.environ.get("KODACT_OPENAI_API_KEY", "")
    if not base or not key:
        sys.exit("Set KODACT_OPENAI_BASE_URL and KODACT_OPENAI_API_KEY as for kodact.")
    url = base + "/chat/completions"

    # What the endpoint lists for this model (some gateways state the levels it takes).
    req = urllib.request.Request(base + "/models", headers={"Authorization": f"Bearer {key}"})
    try:
        ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE") or None)
        with urllib.request.urlopen(req, timeout=30, context=ctx) as r:
            listed = [m for m in json.loads(r.read()).get("data", []) if m.get("id") == a.model]
        print("model listing:", json.dumps(listed[0])[:600] if listed else f"{a.model} not listed")
    except Exception as e:
        print(f"model listing: unavailable ({type(e).__name__})")

    levels = [x for x in a.levels.split(",") if x]
    settings = [("no level", {})]
    settings += [(f"reasoning_effort={lv}", {"reasoning_effort": lv}) for lv in levels]
    settings += [(f"reasoning.effort={lv}", {"reasoning": {"effort": lv}}) for lv in levels]

    rows = []
    for name, extra in settings:
        runs = []
        for _ in range(a.repeat):
            body = {"model": a.model, "messages": [{"role": "user", "content": QUESTION}],
                    "max_tokens": a.max_tokens, **extra}
            status, resp, secs = post(url, key, body, a.timeout)
            if status != 200:
                text = resp if isinstance(resp, str) else json.dumps(resp)
                print(f"{name:28} HTTP {status} after {secs:.1f}s: {text[:300]}")
                runs = None
                break
            f = facts(resp)
            f["secs"] = secs
            runs.append(f)
            print(f"{name:28} reasoning_tokens={f['reasoning_tokens']} output_tokens={f['output_tokens']} "
                  f"reasoning_chars={f['reasoning_chars']} {secs:.1f}s answer={f['answer']!r}")
        if runs:
            med = lambda k: statistics.median([r[k] or 0 for r in runs])  # noqa: E731
            rows.append((name, med("reasoning_tokens"), med("output_tokens"), med("reasoning_chars"), med("secs")))

    print("\nmedians")
    print(f"{'setting':28} {'reasoning tok':>13} {'output tok':>10} {'reasoning ch':>12} {'seconds':>8}")
    for name, rt, ot, rc, s in rows:
        print(f"{name:28} {rt:>13.0f} {ot:>10.0f} {rc:>12.0f} {s:>8.1f}")
    base_row = next((r for r in rows if r[0] == "no level"), None)
    if base_row:
        print("\nA form is honoured when its levels move the reasoning (tokens, or characters where the endpoint "
              "reports no reasoning tokens) away from 'no level' and from each other; refused forms show HTTP errors.")


if __name__ == "__main__":
    main()
