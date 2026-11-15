"""Hold a trial while the model endpoint doesn't answer.

kodact_agent.py waits here first thing in `install()`: Harbor's agent setup, before the agent's
own time starts.

On 2026-11-15 the model gateway stopped answering at about 06:00 UTC, first with "Name does not
resolve", then with "tls handshake eof". Harbor kept starting trials, and each kodact run waited
the outage out, as an unattended run should (up to 30 tries, a minute apart). Ten trials spent
2 h 38 min without a single reply. Every one of them started after this machine itself could no
longer resolve the gateway (their containers got no /etc/hosts pin), so one request before each
trial would have said so.

- **The request.** It is `probe_model.ask()`: a reply of 100,000,000 tokens, which the server
  refuses at once without generating anything, so it costs no tokens. It exercises the name lookup,
  TLS, the key, the gateway and the server behind it, through this machine's proxy settings,
  trusting KODACT_HARBOR_CA_BUNDLE as the containers do.
- **Down.** The endpoint is down when nothing answers (a name that doesn't resolve, a refused
  connection, TLS, a timeout), or when the gateway says the server behind it is unavailable
  (HTTP 502, 503 or 504).
- **Up.** Any other answer means up: usually a 400 refusing the length. A 401 is a key to fix,
  which kodact reports at once.
- **One gate per Harbor process.**
  - One request at a time, and a success is trusted for 30 s, so trials that start together send
    one request.
  - While the endpoint is down, one request a minute, from whichever waiting trial is due.
  - A waiting trial holds its slot, so no other trial starts. When the endpoint answers, they all
    go on.
- **No limit of its own.** Harbor's limit for the agent's setup ends a wait that lasts too long
  (run.sh triples it). report.py then counts the trial as "didn't run": outside the score, and in
  its list of trials to run again.

KODACT_HARBOR_GATE=0 (or off, none, no, false) turns it off. Only an OpenAI-compatible endpoint
(KODACT_OPENAI_BASE_URL) is checked, because the request is probe_model's.
"""

import asyncio
import os
import sys
import time
from collections.abc import Awaitable, Callable, Mapping

from probe_model import ask

OFF = ("0", "off", "none", "no", "false")
# What a gateway answers when the server behind it is down or overloaded.
DOWN = (502, 503, 504)
# Seconds a success is trusted: trials that start together send one request.
REUSE_S = 30.0
# While the endpoint is down: one request this often, from whichever waiting trial is due.
RETRY_S = 60.0
# Seconds one request may take.
TIMEOUT_S = 20.0


def why_down(status: int | None, text: str) -> str:
    """Why the endpoint is down, from `probe_model.ask()`'s answer; "" when it answered."""
    if status is None:
        return " ".join(text.split())[:200] or "no answer"
    if status in DOWN:
        return f"HTTP {status}"
    return ""


def fmt(seconds: float) -> str:
    s = int(seconds)
    return f"{s // 60}m{s % 60:02d}s" if s >= 60 else f"{s}s"


class Gate:
    """One endpoint, for every trial of this process. `probe` asks it (blocking) and says why it is
    down, or "" when it answered."""

    def __init__(
        self,
        probe: Callable[[], str],
        *,
        retry_s: float = RETRY_S,
        reuse_s: float = REUSE_S,
        clock: Callable[[], float] = time.monotonic,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ):
        self._probe = probe
        self._retry_s = retry_s
        self._reuse_s = reuse_s
        self._clock = clock
        self._sleep = sleep
        self._lock = asyncio.Lock()
        self._asked_at: float | None = None
        self._why = ""

    async def wait_open(self, note: Callable[[str], None] = lambda _line: None) -> float:
        """Return once the endpoint answers, at once while a recent request said so. `note` hears
        when this trial starts waiting and when it goes on. Returns the seconds waited."""
        started = self._clock()
        waiting = False
        while True:
            async with self._lock:
                trusted = self._retry_s if self._why else self._reuse_s
                if self._asked_at is None or self._clock() - self._asked_at >= trusted:
                    self._why = await asyncio.to_thread(self._probe)
                    self._asked_at = self._clock()
                why = self._why
                due = self._retry_s - (self._clock() - self._asked_at)
            if not why:
                spent = self._clock() - started
                if waiting:
                    note(f"the model endpoint answers again: going on after {fmt(spent)}")
                return spent
            if not waiting:
                waiting = True
                note(f"the model endpoint doesn't answer ({why}): waiting, and asking again every {fmt(self._retry_s)}")
            await self._sleep(max(1.0, due))


_gates: dict[tuple[str, str], Gate] = {}


def gate_for(base: str, key: str | None, model: str) -> Gate:
    """The gate of this endpoint and model, made at its first use."""
    gate = _gates.get((base, model))
    if gate is None:
        gate = _gates[(base, model)] = Gate(lambda: why_down(*ask(base, key, model, timeout=TIMEOUT_S)))
    return gate


async def wait_for_endpoint(model: str, label: str = "", env: Mapping[str, str] = os.environ) -> tuple[float, list[str]]:
    """Wait until the run's endpoint answers (see the module's doc): the seconds waited, and what
    was said about it, also printed on stderr. Nothing to wait for when the gate is off, there is no
    model, or the endpoint isn't OpenAI-compatible."""
    if env.get("KODACT_HARBOR_GATE", "").strip().lower() in OFF:
        return 0.0, []
    base = env.get("KODACT_OPENAI_BASE_URL", "").strip().rstrip("/")
    if not base or not model:
        return 0.0, []
    said: list[str] = []

    def note(line: str) -> None:
        said.append(line)
        print(f"{label + ': ' if label else ''}{line}", file=sys.stderr, flush=True)

    waited = await gate_for(base, env.get("KODACT_OPENAI_API_KEY"), model).wait_open(note)
    return waited, said
