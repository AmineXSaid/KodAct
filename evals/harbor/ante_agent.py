"""Ante as a Harbor installed agent (Harbor 0.24), modelled on kodact_agent.py.

Each trial gets what a KodAct trial gets, so the two harnesses are compared on the same
Terminal-Bench 2.1 setup: the wait for the model endpoint in setup (endpoint_gate.py), the
endpoint's host pinned in /etc/hosts, the uv, pip and apt caches, the extra CAs, the Hugging Face
Hub address, and a uv for the tests after the run.

Ante runs as its own Harbor adapter runs it (ante-harbor/ante_agent.py), JSON output included: its
events go to `agent/ante.jsonl`, which report.py and watch.py read through ante_stream.py (turns,
tokens, tool calls, and the reply a timed-out run was still writing). Its stderr goes to
`agent/ante.stderr.txt`.
"""

import asyncio
import json
import os
import shlex
import socket
from pathlib import Path
from urllib.parse import urlparse

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from endpoint_gate import wait_for_endpoint
from kodact_agent import (
    REMOTE_CA,
    TRUST_CA,
    container_url,
    hf_env,
    pinned_hosts,
    pip_cache_env,
    provide_uv,
    record_run,
    settings,
    task_time_limit,
    upload_apt_cache,
    upload_pip_cache,
    upload_uv_cache,
    uv_for_tests,
)

ANTE_BIN = Path(os.environ.get("ANTE_BIN", str(Path.home() / ".ante/bin/ante")))
REMOTE_BIN = "/usr/local/bin/ante"
LOG_DIR = "/logs/agent"
STREAM_LOG = "ante.jsonl"
STDERR_LOG = "ante.stderr.txt"
DEFAULT_ANTE_ARGS = "--yolo --no-session-save --no-skills"
FALLBACK_HOSTS = {"gpt.technica-engineering.net": "10.21.3.57"}


def ante_flags(model: str, extra: str) -> list[str]:
    """Ante's flags: its JSON events on stdout, whatever ANTE_HARBOR_ARGS says, unless it picks a format."""
    flags = ["--provider", "openai-compatible", "--model", model, *shlex.split(extra)]
    if "--output-format" not in flags and not any(f.startswith("--output-format=") for f in flags):
        flags += ["--output-format", "json"]
    return flags


class AnteAgent(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "ante"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        if not ANTE_BIN.is_file():
            raise RuntimeError(f"{ANTE_BIN} not found")
        # As for KodAct: a trial waits for the model endpoint here, in Harbor's setup, not in its time.
        logs = getattr(self, "logs_dir", None)
        model = (self.model_name or "").split("/", 1)[-1]
        self.endpoint_wait = await wait_for_endpoint(model, Path(logs).parent.name if logs else "")
        await environment.upload_file(ANTE_BIN, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN}")
        hosts = pinned_hosts()
        host = urlparse(os.environ.get("OPENAI_COMPATIBLE_BASE_URL", "")).hostname
        if host and host not in {name for _, name in hosts}:
            try:
                ip = socket.gethostbyname(host)
            except OSError:
                ip = FALLBACK_HOSTS.get(host)
            if ip:
                hosts.append((ip, host))
        if hosts:
            lines = "\n".join(f"{ip} {name}" for ip, name in hosts)
            await self.exec_as_root(environment, command=f"printf '%s\\n' {shlex.quote(lines)} >> /etc/hosts")
        await upload_uv_cache(self, environment)
        await upload_pip_cache(self, environment)
        await upload_apt_cache(self, environment)
        uv_for_tests()  # a KODACT_HARBOR_UV_BIN that isn't a file stops the job here, not after a run
        ca = os.environ.get("KODACT_HARBOR_CA_BUNDLE")
        if ca:
            if not Path(ca).is_file():
                raise RuntimeError(f"KODACT_HARBOR_CA_BUNDLE: {ca} not found")
            await environment.upload_file(Path(ca).resolve(), REMOTE_CA)  # docker cp copies a symlink as a link
            await self.exec_as_root(environment, command=TRUST_CA)

    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        model = (self.model_name or "").split("/", 1)[-1] or "quick-thinking"
        env = {
            "OPENAI_COMPATIBLE_BASE_URL": container_url(os.environ["OPENAI_COMPATIBLE_BASE_URL"]),
            "OPENAI_COMPATIBLE_API_KEY": os.environ["OPENAI_COMPATIBLE_API_KEY"],
            "SSL_CERT_FILE": "/etc/ssl/certs/ca-certificates.crt",
            "ANTE_TASK": instruction,
        }
        env.update(pip_cache_env())
        env.update(hf_env())
        flags = ante_flags(model, os.environ.get("ANTE_HARBOR_ARGS", DEFAULT_ANTE_ARGS))
        meta = {
            "harness": "ante",
            "model": model,
            "served_model": os.environ.get("KODACT_HARBOR_SERVED_MODEL"),
            "served_window": os.environ.get("KODACT_HARBOR_SERVED_WINDOW"),
            "flags": [*flags, "-p"],
            "time_limit_s": task_time_limit(),
            "settings": settings(env),
        }
        waited, said = getattr(self, "endpoint_wait", (0.0, []))
        if said:
            meta["endpoint_wait_s"] = round(waited)
            meta["endpoint_gate"] = said
        command = (
            f"mkdir -p {LOG_DIR}; "
            + record_run("ANTE_TASK", meta)
            + f'{REMOTE_BIN} {shlex.join(flags)} -p "$ANTE_TASK" < /dev/null '
            f"> {LOG_DIR}/{STREAM_LOG} 2> {LOG_DIR}/{STDERR_LOG}"
        )
        try:
            await self.exec_as_agent(environment, command=command, env=env)
        finally:
            # As for KodAct: after the agent, also when it ran out of time, the tests get a uv.
            try:
                await asyncio.wait_for(provide_uv(self, environment), timeout=120)
            except Exception:  # the tests then fetch uv themselves, as without this
                pass

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Tokens and turns from Ante's events, as report.py counts them."""
        from ante_stream import AnteStream

        path = self.logs_dir / STREAM_LOG
        if not path.is_file():
            return
        stream, result = AnteStream(), None
        for line in path.read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, dict):
                result = next((e for e in stream.feed(event) if e["type"] == "result"), result)
        tokens = stream.tokens
        context.n_input_tokens = tokens["input_tokens"] + tokens["cache_read_input_tokens"]
        context.n_cache_tokens = tokens["cache_read_input_tokens"]
        context.n_output_tokens = tokens["output_tokens"]
        context.metadata = {
            "num_turns": (result or {}).get("num_turns", stream.turns),
            "subtype": (result or {}).get("subtype"),
            "stop_reason": (result or {}).get("stop_reason"),
        }
