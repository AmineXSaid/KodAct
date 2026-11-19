"""Pi (pi-coding-agent) as a Harbor installed agent, next to KodAct, Forge Code and Ante.

Harbor has a Pi agent of its own (`-a pi`), but it installs Node and Pi from the
internet inside every task container, and it has none of the network handling
of kodact_agent.py. This one copies a bundle built on this machine (a Linux x64
`node` binary and Pi's npm package, made by `run-pi.sh`) into the container and
runs Pi once in print mode, against the same OpenAI-compatible endpoint as
KodAct, so the harnesses can be compared on the same model and tasks.

Configuration comes from the environment of the `harbor` process:
- `PI_BUNDLE`: the bundle to copy (`run-pi.sh` builds it and sets this).
- `KODACT_OPENAI_BASE_URL`, `KODACT_OPENAI_API_KEY`: the endpoint and key, the
  same ones KodAct uses. Pi gets them through a `models.json` that declares
  one provider, `kodact-endpoint`; the key stays in an environment variable.
- `KODACT_CONTEXT_WINDOW`: the model's context window, as for KodAct (default
  200,000, what KodAct assumes; Pi's own default is 128,000).
- `KODACT_MAX_OUTPUT_TOKENS`: the most tokens one reply may have, as for KodAct
  (default 32,000, KodAct's; Pi's own default for a custom model is 16,384,
  which a reasoning model can spend thinking before its first tool call).
- `PI_HARBOR_API`: Pi's API type for the endpoint (default `openai-completions`).
- `PI_HARBOR_THINKING`: Pi's thinking level (`low`, `high`, ...); unset leaves
  reasoning off, Pi's default for a custom model.
- `PI_MODELS_JSON`: a `models.json` of your own, used as is instead (its
  provider must be `kodact-endpoint`).
- `PI_HARBOR_ARGS`: extra Pi flags for every task.
- `KODACT_HARBOR_HOSTS`, `KODACT_HARBOR_HOST_IP`, `KODACT_HARBOR_CA_BUNDLE`: as for
  KodAct (see kodact_agent.py). Node gets the system CA bundle, extra CAs included,
  as `NODE_EXTRA_CA_CERTS`.

Pi's JSON events go to `pi.jsonl` (without the per-token `message_update` and
`tool_execution_update` events) and its stderr to `pi.stderr.txt`; report.py and
watch.py read them through pi_stream.py. Pi has no time limit of its own:
Harbor stops it at the task's limit. Written against Harbor 0.24.0.
"""

import json
import os
import shlex
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from shell_args import split as split_args
from kodact_agent import REMOTE_CA, TRUST_CA, upload_uv_cache, upload_pip_cache, upload_apt_cache, pip_cache_env, container_url, pinned_hosts, record_run, settings, task_time_limit

REMOTE_DIR = "/opt/pi-harbor"
REMOTE_PI = f"{REMOTE_DIR}/pi"
REMOTE_BUNDLE = "/tmp/pi-harbor.tar.gz"
CONFIG_DIR = "/tmp/pi-harbor-config"
LOG_DIR = "/logs/agent"
PROVIDER = "kodact-endpoint"
KEY_VAR = "PI_ENDPOINT_API_KEY"
# Drops the events Pi sends for every streamed token or line of output: each repeats the whole
# message so far. Run with the bundle's node, so the container needs nothing of its own.
FILTER = (
    'require("readline").createInterface({input: process.stdin}).on("line", l => {'
    ' if (!/^\\{"type":"(message_update|tool_execution_update)"/.test(l)) process.stdout.write(l + "\\n"); })'
)


def models_json(url: str, model: str) -> str:
    own = os.environ.get("PI_MODELS_JSON")
    if own:
        return Path(own).read_text()
    entry = {
        "id": model,
        "contextWindow": int(os.environ.get("KODACT_CONTEXT_WINDOW") or 200_000),
        "maxTokens": int(os.environ.get("KODACT_MAX_OUTPUT_TOKENS") or 32_000),
    }
    if os.environ.get("PI_HARBOR_THINKING", "off") != "off":
        entry["reasoning"] = True
    provider = {
        "baseUrl": url,
        "api": os.environ.get("PI_HARBOR_API", "openai-completions"),
        "apiKey": f"${KEY_VAR}",  # the variable that holds the key; a bare name is sent as the key itself
        "models": [entry],
    }
    return json.dumps({"providers": {PROVIDER: provider}}, indent=2)


class PiAgent(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "pi"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_PI} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        bundle = Path(os.environ.get("PI_BUNDLE", ""))
        if not bundle.is_file():
            raise RuntimeError(f"PI_BUNDLE {bundle} not found: run evals/harbor/run-pi.sh, which builds it")
        await environment.upload_file(bundle, REMOTE_BUNDLE)
        await self.exec_as_root(
            environment,
            command=(
                f"mkdir -p {REMOTE_DIR} && tar -xzf {REMOTE_BUNDLE} -C {REMOTE_DIR} && rm -f {REMOTE_BUNDLE} "
                f"&& chmod -R a+rX {REMOTE_DIR} && {REMOTE_PI} --version"
            ),
        )
        hosts = pinned_hosts()
        if hosts:
            lines = "\n".join(f"{ip} {name}" for ip, name in hosts)
            await self.exec_as_root(environment, command=f"printf '%s\\n' {shlex.quote(lines)} >> /etc/hosts")
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
        model = (self.model_name or "").split("/", 1)[-1]
        url = os.environ.get("KODACT_OPENAI_BASE_URL")
        key = os.environ.get("KODACT_OPENAI_API_KEY")
        if not url or not key:
            raise RuntimeError("KODACT_OPENAI_BASE_URL and KODACT_OPENAI_API_KEY must be set")
        env = {
            KEY_VAR: key,
            "PI_CODING_AGENT_DIR": CONFIG_DIR,
            "PI_OFFLINE": "1",  # no update check or other startup download
            "PI_TASK": instruction,
        }
        env.update(pip_cache_env())
        flags = ["-p", "--mode", "json", "--no-session", "--provider", PROVIDER, "--model", model]
        thinking = os.environ.get("PI_HARBOR_THINKING", "off")
        if thinking != "off":
            flags += ["--thinking", thinking]
        flags += split_args(os.environ.get("PI_HARBOR_ARGS", ""))
        config = models_json(container_url(url), model)
        meta = {
            "harness": "pi",
            "model": model,
            "flags": flags,
            "time_limit_s": task_time_limit(),
            "settings": settings(env),
            "models_json": {
                name: {**p, "apiKey": "<set>"} for name, p in (json.loads(config).get("providers") or {}).items()
            },
        }
        # Node trusts its own CAs only: give it the system bundle, which TRUST_CA added the extra CAs to.
        trust = ""
        if os.environ.get("KODACT_HARBOR_CA_BUNDLE"):
            trust = (
                "for f in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem "
                f'{REMOTE_CA}; do if [ -r "$f" ]; then export NODE_EXTRA_CA_CERTS="$f"; break; fi; done; '
            )
        command = (
            f"set -o pipefail; mkdir -p {LOG_DIR} {CONFIG_DIR}; {trust}"
            f"printf '%s\\n' {shlex.quote(config)} > {CONFIG_DIR}/models.json; "
            + record_run("PI_TASK", meta)
            + f'{REMOTE_PI} {shlex.join(flags)} "$PI_TASK" < /dev/null 2> {LOG_DIR}/pi.stderr.txt '
            f"| {REMOTE_DIR}/node -e {shlex.quote(FILTER)} > {LOG_DIR}/pi.jsonl"
        )
        await self.exec_as_agent(environment, command=command, env=env)

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Tokens and cost from Pi's events, as report.py counts them."""
        from pi_stream import PiStream

        path = self.logs_dir / "pi.jsonl"
        if not path.is_file():
            return
        stream, result = PiStream(), None
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
        context.cost_usd = stream.cost or None
        context.metadata = {
            "num_turns": stream.turns,
            "subtype": (result or {}).get("subtype"),
            "stop_reason": (result or {}).get("stop_reason"),
        }
