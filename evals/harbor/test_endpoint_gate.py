"""Tests for endpoint_gate.py: a trial waits while the model endpoint doesn't answer, and goes on
when it does. Run without Harbor, Docker or a real endpoint.

    python3 -m pytest evals/harbor/test_endpoint_gate.py

What they hold:
- a refusal of the impossible reply (what vLLM answers) means up;
- a refused connection, a name that doesn't resolve, and a gateway's 502/503/504 mean down;
- the trials of one process share their requests: one while it answers (reused for a while), and
  one per retry period while it doesn't;
- each trial says when it starts waiting and when it goes on;
- the gate turned off, or no OpenAI-compatible endpoint, waits for nothing.
"""

import asyncio
import json
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import endpoint_gate
from endpoint_gate import Gate, wait_for_endpoint, why_down
from probe_model import ask

# DeepSeek V4 Flash behind LiteLLM refusing the impossible reply, as on 2026-11-11.
REFUSAL = (
    '{"detail":"litellm.BadRequestError: Hosted_vllmException - max_tokens=100000000 cannot be greater than '
    'max_model_len=524288. Received Model Group=deepseek-v4-flash"}'
)


class Server:
    """A local endpoint answering every request with `status` and `body`."""

    def __init__(self, status: int, body: str):
        outer = self
        self.requests: list[dict] = []

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                outer.requests.append(json.loads(self.rfile.read(n) or b"{}"))
                data = body.encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_address[1]}/v1"
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def close(self):
        self.httpd.shutdown()
        self.httpd.server_close()


def closed_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def test_a_refusal_of_the_impossible_reply_means_the_endpoint_answers():
    server = Server(400, REFUSAL)
    try:
        status, text = ask(server.url, "k", "deep-thinking", timeout=5)
        assert (status, why_down(status, text)) == (400, "")
        assert server.requests[0]["max_tokens"] == 100_000_000, "nothing is generated"
    finally:
        server.close()


def test_nothing_answering_or_a_gateway_without_its_server_means_down():
    status, text = ask(f"http://127.0.0.1:{closed_port()}/v1", None, "m", timeout=5)
    assert status is None and why_down(status, text), text
    assert why_down(None, "[Errno -2] Name does not resolve") == "[Errno -2] Name does not resolve"
    for code in (502, 503, 504):
        server = Server(code, "upstream unavailable")
        try:
            assert why_down(*ask(server.url, None, "m", timeout=5)) == f"HTTP {code}"
        finally:
            server.close()
    # A key to fix is not an outage: kodact reports it at once.
    assert why_down(401, '{"error": "invalid api key"}') == ""


class FakeTime:
    def __init__(self):
        self.now = 0.0

    def clock(self) -> float:
        return self.now

    async def sleep(self, seconds: float) -> None:
        self.now += seconds
        await asyncio.sleep(0)


def scripted(answers: list[str]):
    """A probe that gives these answers in turn ("" is up), then up; and how often it was asked."""
    asked = []

    def probe() -> str:
        asked.append(1)
        return answers[len(asked) - 1] if len(asked) <= len(answers) else ""

    return probe, asked


def test_a_trial_waits_while_the_endpoint_is_down_and_says_so():
    t = FakeTime()
    probe, asked = scripted(["Name does not resolve", "tls handshake eof", ""])
    gate = Gate(probe, retry_s=60, reuse_s=30, clock=t.clock, sleep=t.sleep)
    said: list[str] = []
    waited = asyncio.run(gate.wait_open(said.append))
    assert len(asked) == 3 and waited == 120
    assert said[0] == "the model endpoint doesn't answer (Name does not resolve): waiting, and asking again every 1m00s"
    assert said[1] == "the model endpoint answers again: going on after 2m00s"


def test_trials_share_the_requests():
    t = FakeTime()
    probe, asked = scripted(["down", "down", ""])
    gate = Gate(probe, retry_s=60, reuse_s=30, clock=t.clock, sleep=t.sleep)

    async def three_trials():
        return await asyncio.gather(*(gate.wait_open() for _ in range(3)))

    asyncio.run(three_trials())
    assert len(asked) == 3, "one request per minute while down, whichever trial is due"
    # Up now: trials that start within the next 30 s send nothing; a later one asks again.
    asyncio.run(gate.wait_open())
    assert len(asked) == 3
    t.now += 31
    asyncio.run(gate.wait_open())
    assert len(asked) == 4


def test_an_answering_endpoint_costs_one_request_and_no_wait():
    t = FakeTime()
    probe, asked = scripted([])
    gate = Gate(probe, clock=t.clock, sleep=t.sleep)
    said: list[str] = []
    assert asyncio.run(gate.wait_open(said.append)) == 0 and len(asked) == 1 and said == []


def test_the_gate_off_or_no_openai_endpoint_waits_for_nothing(monkeypatch):
    def fail(*_args, **_kw):
        raise AssertionError("asked the endpoint")

    monkeypatch.setattr(endpoint_gate, "ask", fail)
    endpoint_gate._gates.clear()
    url = f"http://127.0.0.1:{closed_port()}/v1"
    assert asyncio.run(wait_for_endpoint("m", env={"KODACT_OPENAI_BASE_URL": url, "KODACT_HARBOR_GATE": "0"})) == (0.0, [])
    assert asyncio.run(wait_for_endpoint("m", env={"KODACT_BASE_URL": "https://example.invalid"})) == (0.0, [])
    assert asyncio.run(wait_for_endpoint("", env={"KODACT_OPENAI_BASE_URL": url})) == (0.0, [])


def test_wait_for_endpoint_goes_on_at_once_when_it_answers():
    server = Server(400, REFUSAL)
    endpoint_gate._gates.clear()
    try:
        waited, said = asyncio.run(wait_for_endpoint("deep-thinking", "t1", env={"KODACT_OPENAI_BASE_URL": server.url}))
        assert waited < 5 and said == [] and len(server.requests) == 1
    finally:
        server.close()
