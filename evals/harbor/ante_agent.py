"""Ante as a Harbor installed agent (Harbor 0.24), modelled on kodact_agent.py."""

import os
import shlex
import socket
from pathlib import Path
from urllib.parse import urlparse

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from kodact_agent import REMOTE_CA, TRUST_CA, upload_uv_cache, upload_pip_cache, upload_apt_cache, pip_cache_env, container_url, record_run, settings, task_time_limit

ANTE_BIN = Path(os.environ.get("ANTE_BIN", str(Path.home() / ".ante/bin/ante")))
REMOTE_BIN = "/usr/local/bin/ante"
LOG_DIR = "/logs/agent"
DEFAULT_ANTE_ARGS = "--yolo --no-session-save --no-skills"
FALLBACK_HOSTS = {"gpt.technica-engineering.net": "10.21.3.57"}


class AnteAgent(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "ante"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        if not ANTE_BIN.is_file():
            raise RuntimeError(f"{ANTE_BIN} not found")
        await environment.upload_file(ANTE_BIN, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN}")
        host = urlparse(os.environ.get("OPENAI_COMPATIBLE_BASE_URL", "")).hostname
        if host:
            try:
                ip = socket.gethostbyname(host)
            except OSError:
                ip = FALLBACK_HOSTS.get(host)
            if ip:
                await self.exec_as_root(
                    environment, command=f"printf '%s\\n' {shlex.quote(f'{ip} {host}')} >> /etc/hosts"
                )
        await upload_uv_cache(self, environment)
        await upload_pip_cache(self, environment)
        await upload_apt_cache(self, environment)
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
        # Ante's own Harbor agent runs with these (plus JSON output, left out to keep logs readable).
        extra = os.environ.get("ANTE_HARBOR_ARGS", DEFAULT_ANTE_ARGS)
        flags = ["--provider", "openai-compatible", "--model", model, *shlex.split(extra), "-p"]
        meta = {"harness": "ante", "model": model, "flags": flags, "time_limit_s": task_time_limit(), "settings": settings(env)}
        command = (
            f"mkdir -p {LOG_DIR}; "
            + record_run("ANTE_TASK", meta)
            + f'{REMOTE_BIN} --provider openai-compatible --model {shlex.quote(model)} {extra} -p "$ANTE_TASK" '
            f"> {LOG_DIR}/ante.stdout.txt 2> {LOG_DIR}/ante.stderr.txt"
        )
        await self.exec_as_agent(environment, command=command, env=env)

    def populate_context_post_run(self, context: AgentContext) -> None:
        pass
