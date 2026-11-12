"""KodAct as a Harbor installed agent, for Terminal-Bench runs.

Harbor starts each task's container, calls `install()`, then `run()` with the
task instruction, and grades the container afterwards. This agent copies a
static `kodact` binary into the container and runs it once in print mode.

Configuration comes from the environment of the `harbor` process:

- `KODACT_STATIC_BIN`: the static binary to copy (default:
  `target-static/release/kodact`, built by `evals/harbor/build-static.sh`).
- Every `KODACT_*` variable (endpoint, key, context window, ...) is passed to
  `kodact` inside the container. The key is never put on the command line.
- `KODACT_HARBOR_ARGS`: extra `kodact` flags for every task (for example
  `--max-turns 100`).
- `KODACT_HARBOR_AUTONOMOUS`: `kodact` runs with `--autonomous` (no question tools,
  attempt before giving up); `0` turns it off, for A/B runs.
- `KODACT_HARBOR_TIME_LIMIT`: seconds for `--max-time`. By default the task's own
  agent time limit is used (read from the running trial), minus a margin, so
  the model knows its deadline, is warned before it, and KodAct stops at it
  with a result before Harbor's own limit.
- `KODACT_HARBOR_HOSTS`: extra host names (comma-separated) to resolve on this
  machine and pin in each container's `/etc/hosts`. The endpoint's host
  (`KODACT_OPENAI_BASE_URL` or `KODACT_BASE_URL`) is always pinned, so an
  endpoint behind company DNS works from task containers;
  `KODACT_HARBOR_HOSTS=none` turns pinning off.
- `KODACT_HARBOR_HOST_IP`: a self-hosted endpoint on this machine (`localhost`,
  `127.0.0.1`) means the container itself inside a task container, so such a URL
  is rewritten to this machine's address, found automatically or set here. The
  server must listen on that address (`0.0.0.0`), not only on `127.0.0.1`.
- `KODACT_HARBOR_CA_BUNDLE`: a PEM file of extra certificate authorities to
  trust inside the task container (behind a TLS-inspecting proxy); the proxy's
  certificates alone are enough, the container's own roots are kept. This
  changes the task environment: scores are for local comparisons only.
- `KODACT_HARBOR_HF_ENDPOINT`: another Hugging Face Hub address (a mirror, where
  a proxy blocks huggingface.co), given to the agent here and to the tests by
  verifier-ca.sh as `HF_ENDPOINT`. Public test data only: the mirror sees every
  request.

The model comes from `harbor run -m provider/name`; the provider part is
dropped, so `-m openai/deep-thinking` and `-m deep-thinking` both run `kodact --model deep-thinking`.

Run it through `evals/harbor/run.sh`. Written against Harbor 0.24.0.
"""

import inspect
import json
import os
import shlex
import socket
from pathlib import Path
from urllib.parse import urlparse

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

REPO = Path(__file__).resolve().parents[2]
DEFAULT_BIN = REPO / "target-static" / "release" / "kodact"
REMOTE_BIN = "/usr/local/bin/kodact"
# Inside the container; Harbor collects /logs/agent into this trial's logs_dir.
LOG_DIR = "/logs/agent"
STREAM_LOG = "kodact.jsonl"
STDERR_LOG = "kodact.stderr.txt"
# Seconds kept back from the task's limit, so the final report is written before the cut-off.
TIME_MARGIN = 30
REMOTE_CA = "/tmp/kodact-extra-ca.pem"
# A copy outside /tmp, which a task or the agent may clear, for the task's tests
# (`verifier-ca.sh` points them here): a test that finds no bundle trusts nothing.
KEPT_CA = "/usr/local/share/kodact/ca-bundle.pem"
# Where KODACT_HARBOR_UV_CACHE goes in the container; verifier-ca.sh points the tests' uv here.
KEPT_UV_CACHE = "/usr/local/share/kodact/uv-cache"
# A wheelhouse of what an agent may `pip install` (Torch, made by warm_pip_cache.py); pip_cache_env()
# points pip here, so the agent's own install comes from disk, not a slow proxy.
KEPT_PIP = "/usr/local/share/kodact/pip-wheelhouse"
# Trust REMOTE_CA in the container: in the system store (now, and for any later
# `update-ca-certificates`, e.g. when a test script installs curl), in existing bundle files,
# and for uv, which test scripts use to install their tools.
TRUST_CA = r"""
src=%s
[ -s "$src" ] || { echo "KODACT_HARBOR_CA_BUNDLE did not arrive in the container: $src is missing or empty" >&2; exit 1; }
mkdir -p /usr/local/share/ca-certificates
if command -v awk >/dev/null 2>&1; then
  awk '/-----BEGIN CERTIFICATE-----/{n++; f=sprintf("/usr/local/share/ca-certificates/kodact-extra-%%03d.crt", n)} n>0{print > f}' "$src"
else
  cp "$src" /usr/local/share/ca-certificates/kodact-extra.crt
fi
if command -v update-ca-certificates >/dev/null 2>&1; then update-ca-certificates >/dev/null 2>&1 || true; fi
if [ -d /etc/pki/ca-trust/source/anchors ]; then
  cp "$src" /etc/pki/ca-trust/source/anchors/kodact-extra.pem
  command -v update-ca-trust >/dev/null 2>&1 && update-ca-trust >/dev/null 2>&1 || true
fi
for b in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem; do
  if [ -f "$b" ]; then cat "$src" >> "$b"; fi
done
mkdir -p /etc/uv && printf 'native-tls = true\n' > /etc/uv/uv.toml
# The copy the tests are pointed at (verifier-ca.sh) replaces each tool's own roots, so it holds the
# container's roots and the extra ones: a bundle of only a proxy's certificates would leave every
# host the proxy lets through untouched (download.pytorch.org behind Zscaler) untrusted.
kept=%s
mkdir -p "$(dirname "$kept")"
base=""
for b in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem; do
  if [ -s "$b" ]; then base="$b"; break; fi
done
if [ -z "$base" ] && command -v python3 >/dev/null 2>&1; then
  base=$(python3 -c 'import certifi; print(certifi.where())' 2>/dev/null || true)
fi
if [ -n "$base" ] && [ -s "$base" ]; then cat "$base" "$src" > "$kept"; else cp "$src" "$kept"; fi
chmod 644 "$kept"
""" % (REMOTE_CA, KEPT_CA)


async def upload_uv_cache(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Copy KODACT_HARBOR_UV_CACHE (made by warm_uv_cache.py) into the container for the task's tests.

    The tests then take Torch and the like from disk instead of the network (see verifier-ca.sh).
    """
    cache = os.environ.get("KODACT_HARBOR_UV_CACHE")
    if not cache:
        return
    path = Path(cache).expanduser()
    if not path.is_dir():
        raise RuntimeError(f"KODACT_HARBOR_UV_CACHE: {cache} is not a directory (make it with warm_uv_cache.py)")
    await agent.exec_as_root(environment, command=f"mkdir -p {KEPT_UV_CACHE}")
    await environment.upload_dir(path.resolve(), KEPT_UV_CACHE)
    # Whatever user the tests run as, their uv takes locks and writes in its cache.
    await agent.exec_as_root(environment, command=f"chmod -R a+rwX {KEPT_UV_CACHE}")


async def upload_pip_cache(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Copy KODACT_HARBOR_PIP_CACHE (a wheelhouse made by warm_pip_cache.py) into the container.

    pip_cache_env() then points the agent's own `pip install` at it, so a package the task does not
    provide (Torch) installs from disk instead of over the network. The task still withholds it; only
    the download speed changes. Applies to every agent, so comparisons stay even.
    """
    cache = os.environ.get("KODACT_HARBOR_PIP_CACHE")
    if not cache:
        return
    path = Path(cache).expanduser()
    if not path.is_dir():
        raise RuntimeError(f"KODACT_HARBOR_PIP_CACHE: {cache} is not a directory (make it with warm_pip_cache.py)")
    await agent.exec_as_root(environment, command=f"mkdir -p {KEPT_PIP}")
    await environment.upload_dir(path.resolve(), KEPT_PIP)
    await agent.exec_as_root(environment, command=f"chmod -R a+rX {KEPT_PIP}")


def pip_cache_env() -> dict[str, str]:
    """Environment that makes the agent's `pip install` prefer the uploaded wheelhouse.

    PIP_FIND_LINKS adds it as a source alongside any index the agent names, so a cached wheel is
    used without the network and anything missing still falls back to the index. The longer timeout
    and retries help that fallback survive a slow proxy. Empty when no cache was provided.
    """
    if not os.environ.get("KODACT_HARBOR_PIP_CACHE"):
        return {}
    return {"PIP_FIND_LINKS": KEPT_PIP, "PIP_DEFAULT_TIMEOUT": "600", "PIP_RETRIES": "10"}


def hf_env() -> dict[str, str]:
    """With KODACT_HARBOR_HF_ENDPOINT: the agent's Hugging Face tools use that Hub address, as the
    tests do (verifier-ca.sh), so the agent can fetch the files the tests will. With
    KODACT_HARBOR_CA_BUNDLE, requests and curl trust the kept bundle there too. Empty otherwise.
    """
    endpoint = os.environ.get("KODACT_HARBOR_HF_ENDPOINT", "").strip().rstrip("/")
    if not endpoint:
        return {}
    env = {"HF_ENDPOINT": endpoint, "HF_HUB_DISABLE_XET": "1"}
    if os.environ.get("KODACT_HARBOR_CA_BUNDLE"):
        env.update({var: KEPT_CA for var in ("REQUESTS_CA_BUNDLE", "SSL_CERT_FILE", "CURL_CA_BUNDLE")})
    return env


def pinned_hosts() -> list[tuple[str, str]]:
    """(ip, name) for the endpoint's host and KODACT_HARBOR_HOSTS, as this machine resolves them.

    Task containers use Docker's DNS, which doesn't know names that only company DNS
    resolves; pinning them in /etc/hosts lets kodact reach such an endpoint.
    """
    extra = os.environ.get("KODACT_HARBOR_HOSTS", "")
    if extra.strip().lower() == "none":
        return []
    names = [n.strip() for n in extra.split(",") if n.strip()]
    for var in ("KODACT_OPENAI_BASE_URL", "KODACT_BASE_URL"):
        host = urlparse(os.environ.get(var, "")).hostname
        if host:
            names.append(host)
    out = []
    for name in dict.fromkeys(names):
        if name.lower() in LOOPBACK:
            continue  # rewritten to this machine's address by container_url()
        try:
            socket.inet_aton(name)
            continue  # already an address
        except OSError:
            pass
        try:
            out.append((socket.gethostbyname(name), name))
        except OSError:
            pass  # unresolvable here too: leave it to the container's DNS
    return out


LOOPBACK = ("localhost", "127.0.0.1", "0.0.0.0", "::1")


def host_ip() -> str:
    """This machine's address as task containers can reach it (KODACT_HARBOR_HOST_IP overrides)."""
    ip = os.environ.get("KODACT_HARBOR_HOST_IP")
    if ip:
        return ip
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("10.255.255.255", 1))  # picks the outgoing interface; sends nothing
            return s.getsockname()[0]
    except OSError as e:
        raise RuntimeError(
            f"cannot find this machine's address for a localhost endpoint ({e}); "
            "set KODACT_HARBOR_HOST_IP to an address task containers can reach"
        ) from e


def container_url(url: str) -> str:
    """`url` as seen from a task container: a loopback host becomes this machine's address."""
    parts = urlparse(url)
    if (parts.hostname or "").lower() not in LOOPBACK and not (parts.hostname or "").startswith("127."):
        return url
    ip = host_ip()
    userinfo = parts.netloc.rpartition("@")[0]
    netloc = (f"{userinfo}@" if userinfo else "") + (f"[{ip}]" if ":" in ip else ip)
    netloc += f":{parts.port}" if parts.port else ""
    return parts._replace(netloc=netloc).geturl()


SECRET = ("KEY", "TOKEN", "SECRET", "PASSWORD")


def record_run(task_var: str, meta: dict) -> str:
    """Shell that saves the task and how the agent was started next to its logs, for report.py."""
    return (
        f'printf "%s" "${task_var}" > {LOG_DIR}/instruction.txt; '
        f"printf '%s\\n' {shlex.quote(json.dumps(meta))} > {LOG_DIR}/run.json; "
    )


def settings(env: dict) -> dict:
    """The settings passed to the agent, with the values of keys and tokens left out."""
    return {k: ("<set>" if any(s in k.upper() for s in SECRET) else v) for k, v in sorted(env.items()) if not k.endswith("_TASK")}


def task_time_limit() -> float | None:
    """The agent time limit of the trial running this agent, in seconds.

    Harbor gives it only to its oracle agent, so it is read from the Trial on the
    call stack (Harbor 0.24 awaits `run()` directly); KODACT_HARBOR_TIME_LIMIT overrides it.
    """
    override = os.environ.get("KODACT_HARBOR_TIME_LIMIT")
    if override:
        return float(override)
    frame = inspect.currentframe()
    while frame is not None:
        value = getattr(frame.f_locals.get("self"), "_agent_timeout_sec", None)
        if isinstance(value, (int, float)) and value > 0:
            return float(value)
        frame = frame.f_back
    return None


class KodAct(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "kodact"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        binary = Path(os.environ.get("KODACT_STATIC_BIN", DEFAULT_BIN))
        if not binary.is_file():
            raise RuntimeError(f"{binary} not found: run evals/harbor/build-static.sh first")
        await environment.upload_file(binary, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN} && {REMOTE_BIN} --version")
        hosts = pinned_hosts()
        if hosts:
            lines = "\n".join(f"{ip} {name}" for ip, name in hosts)
            await self.exec_as_root(environment, command=f"printf '%s\\n' {shlex.quote(lines)} >> /etc/hosts")
        await upload_uv_cache(self, environment)
        await upload_pip_cache(self, environment)
        ca = os.environ.get("KODACT_HARBOR_CA_BUNDLE")
        if ca:
            if not Path(ca).is_file():
                raise RuntimeError(f"KODACT_HARBOR_CA_BUNDLE: {ca} not found")
            await environment.upload_file(Path(ca).resolve(), REMOTE_CA)  # docker cp copies a symlink as a link
            await self.exec_as_root(environment, command=TRUST_CA)

    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        model = (self.model_name or "").split("/", 1)[-1]
        env = {
            k: v
            for k, v in os.environ.items()
            if k.startswith("KODACT_") and k != "KODACT_STATIC_BIN" and not k.startswith("KODACT_HARBOR_")
        }
        for var in ("KODACT_OPENAI_BASE_URL", "KODACT_BASE_URL"):
            if env.get(var):
                env[var] = container_url(env[var])
        env["KODACT_TASK"] = instruction
        env.update(pip_cache_env())
        env.update(hf_env())
        flags = ["--dangerously-skip-permissions", "--output-format", "stream-json", "--verbose"]
        if os.environ.get("KODACT_HARBOR_AUTONOMOUS", "1").strip().lower() not in ("0", "false", "no", "off"):
            flags.append("--autonomous")
        if model:
            flags += ["--model", model]
        extra = shlex.split(os.environ.get("KODACT_HARBOR_ARGS", ""))
        limit = task_time_limit()
        if limit and "--max-time" not in extra:
            flags += ["--max-time", str(max(60, int(limit) - TIME_MARGIN))]
        flags += extra
        meta = {
            "harness": "kodact",
            "model": model,
            # What the endpoint said it serves behind `model`, and its window, at job start (run.sh).
            "served_model": os.environ.get("KODACT_HARBOR_SERVED_MODEL"),
            "served_window": os.environ.get("KODACT_HARBOR_SERVED_WINDOW"),
            "flags": flags,
            "time_limit_s": limit,
            "settings": settings(env),
        }
        command = (
            f"mkdir -p {LOG_DIR}; "
            + record_run("KODACT_TASK", meta)
            + f'{REMOTE_BIN} -p "$KODACT_TASK" {shlex.join(flags)} '
            f"> {LOG_DIR}/{STREAM_LOG} 2> {LOG_DIR}/{STDERR_LOG}"
        )
        await self.exec_as_agent(environment, command=command, env=env)

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Tokens and cost from the final `result` line of the stream."""
        path = self.logs_dir / STREAM_LOG
        if not path.is_file():
            return
        result = None
        for line in path.read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, dict) and event.get("type") == "result":
                result = event
        if result is None:
            return
        usage = result.get("usage") or {}
        context.n_input_tokens = usage.get("input_tokens", 0) + usage.get("cache_read_input_tokens", 0)
        context.n_cache_tokens = usage.get("cache_read_input_tokens", 0)
        context.n_output_tokens = usage.get("output_tokens", 0)
        context.cost_usd = result.get("total_cost_usd")
        context.metadata = {
            "num_turns": result.get("num_turns"),
            "subtype": result.get("subtype"),
            "stop_reason": result.get("stop_reason"),
        }
