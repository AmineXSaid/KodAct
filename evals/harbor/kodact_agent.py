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
  verifier-ca.sh as `HF_ENDPOINT`. The run scripts settle it first
  (hf_endpoint.py): unset, a mirror only where huggingface.co is blocked from this
  machine. Public test data only: the mirror sees every request.
- `KODACT_HARBOR_UV_BIN`: the uv put in the container for the task's tests after
  the agent's run, when there is none (provide_uv): a test script whose own
  download of uv fails then runs all the same. Default: this machine's `uv`;
  `none` turns it off.
- `KODACT_HARBOR_GATE`: a trial waits in its setup, before the agent's time starts, while
  the model endpoint doesn't answer (endpoint_gate.py), so an outage doesn't turn every trial
  started during it into a run with no reply; `0` turns it off.
- `KODACT_HARBOR_UV_CACHE`, `KODACT_HARBOR_PIP_CACHE`, `KODACT_HARBOR_APT_CACHE`:
  downloads made once on this machine (warm_*.py), copied in before the agent
  starts: the tests' packages, what the agent pip-installs (Torch), and what it
  apt-installs when a task has no Python (upload_apt_cache). The run scripts make
  what is missing and set each to its folder in ~/.kodact-harbor (caches.py). Each
  task's tests have a uv cache of their own, and a container gets only its task's
  (uv_cache_for). Only the download speed changes; the task still withholds the
  packages.

The model comes from `harbor run -m provider/name`; the provider part is
dropped, so `-m openai/deep-thinking` and `-m deep-thinking` both run `kodact --model deep-thinking`.

Run it through `evals/harbor/run.sh`. Written against Harbor 0.24.0.
"""

import asyncio
import inspect
import json
import os
import re
import shlex
import shutil
import socket
from pathlib import Path
from urllib.parse import urlparse

from endpoint_gate import wait_for_endpoint
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
# The uv binary given to the tests (provide_uv), uploaded here, and what came of it, in the trial's logs.
REMOTE_UV = "/tmp/kodact-uv-for-tests"
UV_LOG = "uv-for-tests.txt"
# Settings that turn something off.
OFF = ("none", "off", "0", "no", "false")
# The tests' own uv, for when their download of it fails. Terminal-Bench test scripts install uv
# with `curl -LsSf https://astral.sh/uv/<version>/install.sh | sh`, `source $HOME/.local/bin/env`,
# then run `uvx ... pytest`. When the download fails (a proxy, a DNS blip: "Could not resolve
# host: releases.astral.sh"), `uvx: command not found` and nothing is judged. This puts a uv
# where that installer would, unless one is there already or it doesn't run in this container
# (another libc or architecture); the installer, when it works, replaces it. A uvx that is
# `uv tool run` and an env file that puts ~/.local/bin on PATH come with it, as the installer's do.
UV_FOR_TESTS = r"""
src=@SRC@
bin="$HOME/.local/bin"
if [ -x "$bin/uv" ] || command -v uvx >/dev/null 2>&1; then
  rm -f "$src"; echo "uv for the tests: one is there already"; exit 0
fi
mkdir -p "$bin" && cp "$src" "$bin/uv.kodact" && chmod 755 "$bin/uv.kodact" && rm -f "$src" || exit 0
if ! version=$("$bin/uv.kodact" --version 2>/dev/null); then
  rm -f "$bin/uv.kodact"; echo "uv for the tests: the uv of the machine running Harbor doesn't run here"; exit 0
fi
mv "$bin/uv.kodact" "$bin/uv"
[ -e "$bin/uvx" ] || { printf '#!/bin/sh\n[ "$1" = --version ] && exec "$(dirname "$0")/uv" --version\nexec "$(dirname "$0")/uv" tool run "$@"\n' > "$bin/uvx"; chmod 755 "$bin/uvx"; }
[ -e "$bin/env" ] || printf '%s\n' '#!/bin/sh' 'case ":${PATH}:" in' '  *:"$HOME/.local/bin":*) ;;' \
  '  *) export PATH="$HOME/.local/bin:$PATH" ;;' 'esac' > "$bin/env"
echo "uv for the tests: $version, from the machine running Harbor"
"""
# A wheelhouse of what an agent may `pip install` (Torch, made by warm_pip_cache.py); pip_cache_env()
# points pip here, so the agent's own install comes from disk, not a slow proxy.
KEPT_PIP = "/usr/local/share/kodact/pip-wheelhouse"
# What an agent may `apt-get install` when a task's container has no Python (made by
# warm_apt_cache.py, one folder per release: ubuntu-noble-amd64). Its .deb files go where apt
# downloads to, which an apt.conf.d line moves here: the Docker images' docker-clean hook empties
# /var/cache/apt/archives after every `apt-get update`. Its package lists go to
# /var/lib/apt/lists only when the image has none (most delete theirs).
KEPT_APT = "/usr/local/share/kodact/apt-archives"
KEPT_APT_LISTS = "/usr/local/share/kodact/apt-lists"
APT_CONF = "/etc/apt/apt.conf.d/00kodact-apt-cache"
APT_LOG = "apt-cache.txt"
# This container's release, as warm_apt_cache.py names its folders; nothing without apt.
APT_RELEASE = r"""
command -v apt-get >/dev/null 2>&1 || exit 0
. /etc/os-release 2>/dev/null || exit 0
printf '%s-%s-%s\n' "${ID:-}" "${VERSION_CODENAME:-}" "$(dpkg --print-architecture 2>/dev/null)"
"""
# No pipes: Harbor runs commands under `set -o pipefail`, where `ls | grep -q` can fail on SIGPIPE.
USE_APT_CACHE = r"""
archives=@ARCHIVES@
lists=@LISTS@
mkdir -p "$archives/partial"
chown _apt:root "$archives/partial" 2>/dev/null || true
chmod 700 "$archives/partial"
printf 'Dir::Cache::Archives "%s/";\n' "$archives" > @CONF@
seeded=""
set -- @APT_LISTS@/*_Packages*
if [ ! -e "$1" ] && [ -d "$lists" ]; then
  mkdir -p @APT_LISTS@ && cp -a "$lists/." @APT_LISTS@/ && seeded=" and its package lists"
fi
rm -rf "$lists"
n=0
for f in "$archives"/*.deb; do [ -e "$f" ] && n=$((n + 1)); done
echo "apt cache: $n packages for @RELEASE@$seeded"
"""
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


def cache_dir(var: str, maker: str) -> Path | None:
    """The folder a cache setting names, or None when it is unset or off (`none`, `off`, ...).
    A setting that names no folder stops the job. The run scripts default each one to its folder
    in ~/.kodact-harbor when that exists (caches.sh)."""
    value = os.environ.get(var, "").strip()
    if not value or value.lower() in OFF:
        return None
    path = Path(value).expanduser()
    if not path.is_dir():
        raise RuntimeError(f"{var}: {value} is not a directory (make it with {maker})")
    return path


def uv_cache_for(task: str, cache: Path) -> Path | None:
    """The uv cache for this task's tests: its own folder, tasks/<task>, which warm_uv_cache.py
    fills with only the packages those tests install. None for a task without one. A cache shared
    by every task (made before the folders existed) is copied into no container: it held
    gigabytes, and copying it took 17-18 minutes, past Harbor's setup limit for four tasks
    (2026-11-17)."""
    own = cache / "tasks" / task
    return own if task and own.is_dir() and any(own.iterdir()) else None


async def upload_uv_cache(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Copy this task's folder of KODACT_HARBOR_UV_CACHE (made by warm_uv_cache.py) into the
    container for its tests, which then take Torch and the like from disk (see verifier-ca.sh).
    """
    path = cache_dir("KODACT_HARBOR_UV_CACHE", "warm_uv_cache.py")
    if path is None:
        return
    task = getattr(environment, "environment_name", None) or ""  # the task's short name in Harbor 0.24
    own = uv_cache_for(task, path)
    if own is None:
        return
    await agent.exec_as_root(environment, command=f"mkdir -p {KEPT_UV_CACHE}")
    await environment.upload_dir(own.resolve(), KEPT_UV_CACHE)
    # Whatever user the tests run as, their uv takes locks and writes in its cache.
    await agent.exec_as_root(environment, command=f"chmod -R a+rwX {KEPT_UV_CACHE}")


async def upload_pip_cache(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Copy KODACT_HARBOR_PIP_CACHE (a wheelhouse made by warm_pip_cache.py) into the container.

    pip_cache_env() then points the agent's own `pip install` at it, so a package the task does not
    provide (Torch) installs from disk instead of over the network. The task still withholds it; only
    the download speed changes. Applies to every agent, so comparisons stay even.
    """
    path = cache_dir("KODACT_HARBOR_PIP_CACHE", "warm_pip_cache.py")
    if path is None:
        return
    await agent.exec_as_root(environment, command=f"mkdir -p {KEPT_PIP}")
    await environment.upload_dir(path.resolve(), KEPT_PIP)
    await agent.exec_as_root(environment, command=f"chmod -R a+rX {KEPT_PIP}")


def apt_release(probe_output: str | None) -> str:
    """This container's release as warm_apt_cache.py names its folders (`ubuntu-noble-amd64`), read
    from APT_RELEASE's output; "" for none, or for anything else (it becomes a path here)."""
    lines = (probe_output or "").strip().splitlines()
    release = lines[-1].strip() if lines else ""
    return release if re.fullmatch(r"[a-z0-9]+-[a-z0-9]+-[a-z0-9]+", release) else ""


async def upload_apt_cache(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Copy the KODACT_HARBOR_APT_CACHE folder for this container's release (made by
    warm_apt_cache.py) into the container, and point apt's downloads at it (USE_APT_CACHE).

    The agent's own `apt-get install python3` then takes the files from disk instead of a slow
    mirror. The task still withholds the packages: the agent still decides to install them, and
    only the download speed changes, for every agent alike. A container without apt, or of a
    release with no folder, gets nothing; which one it was is in the trial's agent logs.
    """
    root = cache_dir("KODACT_HARBOR_APT_CACHE", "warm_apt_cache.py")
    if root is None:
        return
    probe = await agent.exec_as_root(environment, command=APT_RELEASE)
    release = apt_release(getattr(probe, "stdout", None))
    if not release:
        return
    folder = root / release
    if not (folder / "archives").is_dir():
        note = f"apt cache: none for {release} (python3 evals/harbor/warm_apt_cache.py <its image> makes one)"
        await agent.exec_as_root(environment, command=f"mkdir -p {LOG_DIR}; echo {shlex.quote(note)} > {LOG_DIR}/{APT_LOG}")
        return
    await agent.exec_as_root(environment, command=f"mkdir -p {KEPT_APT} {KEPT_APT_LISTS}")
    await environment.upload_dir(folder / "archives", KEPT_APT)
    if (folder / "lists").is_dir():
        await environment.upload_dir(folder / "lists", KEPT_APT_LISTS)
    script = use_apt_cache(release)
    await agent.exec_as_root(environment, command=f"mkdir -p {LOG_DIR}; {{ {script} }} > {LOG_DIR}/{APT_LOG} 2>&1")


def use_apt_cache(release: str, conf: str = APT_CONF, apt_lists: str = "/var/lib/apt/lists") -> str:
    """USE_APT_CACHE for this release; tests point `conf` and `apt_lists` elsewhere."""
    return (
        USE_APT_CACHE.replace("@ARCHIVES@", KEPT_APT)
        .replace("@LISTS@", KEPT_APT_LISTS)
        .replace("@CONF@", conf)
        .replace("@APT_LISTS@", apt_lists)
        .replace("@RELEASE@", release)
    )


def uv_for_tests() -> Path | None:
    """The uv binary given to the tests (provide_uv): KODACT_HARBOR_UV_BIN, else this machine's
    `uv` on PATH; None with KODACT_HARBOR_UV_BIN=none, or when this machine has none."""
    choice = os.environ.get("KODACT_HARBOR_UV_BIN", "").strip()
    if choice.lower() in OFF:
        return None
    found = choice or shutil.which("uv")
    if not found:
        return None
    path = Path(found).expanduser().resolve()  # docker cp copies a symlink as a link
    if not path.is_file():
        if choice:
            raise RuntimeError(f"KODACT_HARBOR_UV_BIN: {choice} is not a file")
        return None
    return path


async def provide_uv(agent: BaseInstalledAgent, environment: BaseEnvironment) -> None:
    """Give the task's tests a uv when the container has none (UV_FOR_TESTS), after the agent's run:
    the agent's environment stays the task's own. What came of it is in the trial's agent logs."""
    uv = uv_for_tests()
    if uv is None:
        return
    await environment.upload_file(uv, REMOTE_UV)
    script = UV_FOR_TESTS.replace("@SRC@", REMOTE_UV)
    await agent.exec_as_root(environment, command=f"mkdir -p {LOG_DIR}; {{ {script} }} > {LOG_DIR}/{UV_LOG} 2>&1")


def pip_cache_env() -> dict[str, str]:
    """Environment that makes the agent's `pip install` prefer the uploaded wheelhouse.

    PIP_FIND_LINKS adds it as a source alongside any index the agent names, so a cached wheel is
    used without the network and anything missing still falls back to the index. The longer timeout
    and retries help that fallback survive a slow proxy. Empty when no cache was provided.
    """
    value = os.environ.get("KODACT_HARBOR_PIP_CACHE", "").strip()
    if not value or value.lower() in OFF:
        return {}
    return {"PIP_FIND_LINKS": KEPT_PIP, "PIP_DEFAULT_TIMEOUT": "600", "PIP_RETRIES": "10"}


def hf_env() -> dict[str, str]:
    """With KODACT_HARBOR_HF_ENDPOINT: the agent's Hugging Face tools use that Hub address, as the
    tests do (verifier-ca.sh), so the agent can fetch the files the tests will. With
    KODACT_HARBOR_CA_BUNDLE, requests and curl trust the kept bundle there too. Empty otherwise.
    """
    endpoint = os.environ.get("KODACT_HARBOR_HF_ENDPOINT", "").strip().rstrip("/")
    if not endpoint or endpoint.lower() in OFF:
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


def model_limits_flags(model: str, extra: list[str]) -> list[str]:
    """kodact's `--settings` giving the run's model its reply limit as `modelLimits`, from
    KODACT_HARBOR_MAX_OUTPUT (settled by max-output.sh). None when there is no limit, no model,
    or the flags given already carry their own `--settings`."""
    limit = os.environ.get("KODACT_HARBOR_MAX_OUTPUT", "").strip()
    if not limit or not model or "--settings" in extra:
        return []
    return ["--settings", json.dumps({"modelLimits": {model: {"maxOutputTokens": int(limit)}}})]


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
        # A trial started while the model endpoint doesn't answer gets no reply at all: it waits
        # here, in Harbor's setup, not in the agent's time (endpoint_gate.py).
        logs = getattr(self, "logs_dir", None)
        model = (self.model_name or "").split("/", 1)[-1]
        self.endpoint_wait = await wait_for_endpoint(model, Path(logs).parent.name if logs else "")
        await environment.upload_file(binary, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN} && {REMOTE_BIN} --version")
        hosts = pinned_hosts()
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
        flags += model_limits_flags(model, extra)
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
        waited, said = getattr(self, "endpoint_wait", (0.0, []))
        if said:  # the trial waited for the endpoint before it started (endpoint_gate.py)
            meta["endpoint_wait_s"] = round(waited)
            meta["endpoint_gate"] = said
        command = (
            f"mkdir -p {LOG_DIR}; "
            + record_run("KODACT_TASK", meta)
            # The task last, after `--`: a task text that starts with a dash ("- You are given…",
            # pytorch-model-recovery) is never read as a flag, whatever kodact's parser does.
            + f'{REMOTE_BIN} -p {shlex.join(flags)} -- "$KODACT_TASK" '
            f"> {LOG_DIR}/{STREAM_LOG} 2> {LOG_DIR}/{STDERR_LOG}"
        )
        try:
            await self.exec_as_agent(environment, command=command, env=env)
        finally:
            # After the agent, also when it failed or ran out of time: the tests run next.
            try:
                await asyncio.wait_for(provide_uv(self, environment), timeout=120)
            except Exception:  # the tests then fetch uv themselves, as without this
                pass

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
