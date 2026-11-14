#!/usr/bin/env python3
"""The Hugging Face Hub address a run's agent and tests use: hf-endpoint.sh settles
KODACT_HARBOR_HF_ENDPOINT with it, before the job is named.

    python3 evals/harbor/hf_endpoint.py     # the address to use, or nothing for huggingface.co

- A URL in KODACT_HARBOR_HF_ENDPOINT is used as given.
- `off` (or `none`, `0`, `no`, `false`): huggingface.co, never a mirror.
- Unset: huggingface.co when this machine reaches it. Where a proxy blocks huggingface.co, a test
  that downloads from it (reshard-c4-data loads C4) fails before it judges anything, every run;
  then hf-mirror.com, when it answers. Said on stderr, and the job gets -hfmirror.

hf-mirror.com is run by a third party, not by Hugging Face: it sees the requests made through it.
Those are the tests' and the agent's downloads of public data; no Hugging Face token is passed
into the containers. Both hosts are reached as the containers reach them: through this
machine's proxy, trusting KODACT_HARBOR_CA_BUNDLE besides the system's certificates.
"""

import os
import ssl
import sys
import urllib.error
import urllib.request
from collections.abc import Callable

HUB = "https://huggingface.co"
MIRROR = "https://hf-mirror.com"
# A public dataset that a Terminal-Bench test loads, and that the mirror is known to serve.
PROBE = "/api/datasets/allenai/c4"
OFF = ("off", "none", "0", "no", "false")


def status(url: str, timeout: float = 15) -> int | None:
    """The HTTP status of a GET of `url`, or None when it can't be reached (name, TLS, timeout)."""
    ctx = ssl.create_default_context()
    bundle = os.path.expanduser(os.environ.get("KODACT_HARBOR_CA_BUNDLE", ""))
    if bundle and os.path.isfile(bundle):
        ctx.load_verify_locations(cafile=bundle)
    request = urllib.request.Request(url, headers={"User-Agent": "kodact-harbor"})
    try:
        with urllib.request.urlopen(request, timeout=timeout, context=ctx) as response:
            return response.status
    except urllib.error.HTTPError as e:
        return e.code
    except (urllib.error.URLError, OSError, ValueError):
        return None


def choose(setting: str | None, fetch: Callable[[str], int | None] = status) -> tuple[str, str]:
    """The address to use ("" for huggingface.co) and a line for the person ("" when nothing to say)."""
    value = (setting or "").strip()
    if value.lower() in OFF:
        return "", ""
    if value:
        return value.rstrip("/"), ""
    hub = fetch(HUB + PROBE)
    if hub == 200:
        return "", ""
    why = f"HTTP {hub}" if hub else "no answer"
    mirror = fetch(MIRROR + PROBE)
    if mirror == 200:
        return MIRROR, (
            f"huggingface.co is blocked from here ({why}): the agent and the tests use {MIRROR}, a mirror "
            "run by a third party, for public data (KODACT_HARBOR_HF_ENDPOINT=off turns this off)"
        )
    return "", (
        f"warning: huggingface.co is blocked from here ({why}), and {MIRROR} doesn't answer either "
        f"({'HTTP ' + str(mirror) if mirror else 'no answer'}): tests that download from the Hub "
        "(reshard-c4-data) will end as verifier errors"
    )


def main() -> None:
    address, note = choose(os.environ.get("KODACT_HARBOR_HF_ENDPOINT"))
    if note:
        print(note, file=sys.stderr)
    print(address)


if __name__ == "__main__":
    main()
