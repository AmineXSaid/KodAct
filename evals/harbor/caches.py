#!/usr/bin/env python3
"""The download caches a Harbor run uses, made first when missing. caches.sh evaluates what this
prints, before the job is named, so the usual command is enough: no setup step.

    eval "$(python3 evals/harbor/caches.py)"     # export/unset lines on stdout, the rest on stderr

Three caches make a slow network's downloads come from disk, for every agent alike: the tests'
packages (KODACT_HARBOR_UV_CACHE), what an agent pip-installs (KODACT_HARBOR_PIP_CACHE) and what
it apt-installs when a task has no Python (KODACT_HARBOR_APT_CACHE). Each is the variable's
folder, else its folder in ~/.kodact-harbor; `off` (or none, 0, no, false) is none. Before a run,
what is missing is made, once, and every run after reuses it:

- pip: Torch (CPU) for Python 3.12 and 3.13, while the wheelhouse holds no wheel.
- apt: ubuntu:24.04 and debian:bookworm while no release is cached, then any Ubuntu or Debian
  release a past trial had none for (its agent/apt-cache.txt says so). Needs Docker, as Harbor does.
- uv: the tests' packages of each task whose tests, in a past job here (jobs/), judged nothing:
  they ran out of time, or failed to fetch what they need (report.py's "verifier error").

KODACT_HARBOR_WARM=0 makes nothing and uses only what is there. A step that fails leaves the run
to go on without what it would have made.
"""

import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OFF = ("off", "none", "0", "no", "false")
# Each cache's variable and its folder in ~/.kodact-harbor.
CACHES = {
    "KODACT_HARBOR_UV_CACHE": "uv-cache",
    "KODACT_HARBOR_PIP_CACHE": "pip-wheelhouse",
    "KODACT_HARBOR_APT_CACHE": "apt-cache",
}
APT_IMAGES = ["ubuntu:24.04", "debian:bookworm"]
APT_DISTROS = ("ubuntu", "debian")  # whose release names are Docker image tags (ubuntu:jammy)
Step = tuple[str, list[str], str, Path]  # (warm script, its arguments, the cache's variable, its folder)


def say(text: str) -> None:
    print(text, file=sys.stderr, flush=True)


def load(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def listed(path: Path) -> set[str]:
    try:
        return set(path.read_text().split())
    except OSError:
        return set()


def folders(environ, home: Path) -> dict[str, Path | None]:
    """Each cache's folder: the variable's, else its own in `home`; None when off."""
    out: dict[str, Path | None] = {}
    for var, name in CACHES.items():
        value = environ.get(var, "").strip()
        if value and value.lower() in OFF:
            out[var] = None
        else:
            out[var] = Path(value).expanduser() if value else home / name
    return out


def unjudged_tasks(jobs: Path) -> set[str]:
    """Tasks whose tests, in a past trial here, judged nothing: they ran out of time, or failed to
    fetch what they need. A wrong answer, or a package database the agent broke, isn't one."""
    from report import tests_unjudged

    tasks = set()
    for trial in jobs.glob("*/*__*"):
        if not trial.is_dir():
            continue
        task = trial.name.rsplit("__", 1)[0]
        if (load(trial / "result.json").get("exception_info") or {}).get("exception_type") == "VerifierTimeoutError":
            tasks.add(task)
            continue
        verifier = trial / "verifier"
        try:
            reward = float((verifier / "reward.txt").read_text().strip())
        except (OSError, ValueError):
            continue
        tests = (load(verifier / "ctrf.json").get("results") or {}).get("tests") or []
        try:
            stdout = (verifier / "test-stdout.txt").read_text(errors="replace")
        except OSError:
            stdout = ""
        if tests_unjudged(reward, tests, stdout) == "verifier error":
            tasks.add(task)
    return tasks


def apt_missing(jobs: Path) -> set[str]:
    """Releases a past trial had no apt cache for: its agent/apt-cache.txt says `none for <release>`."""
    found = set()
    for note in jobs.glob("*/*__*/agent/apt-cache.txt"):
        match = re.search(r"none for ([a-z0-9]+-[a-z0-9]+-[a-z0-9]+)", note.read_text(errors="replace"))
        if match:
            found.add(match.group(1))
    return found


def plan(environ, home: Path, jobs: Path, arch: str) -> tuple[list[Step], dict[str, Path | None]]:
    """What to make before the run, and each cache's folder."""
    where = folders(environ, home)
    steps: list[Step] = []
    pip = where["KODACT_HARBOR_PIP_CACHE"]
    if pip is not None and not any(pip.glob("*.whl")):
        steps.append(("warm_pip_cache.py", [], "KODACT_HARBOR_PIP_CACHE", pip))
    apt = where["KODACT_HARBOR_APT_CACHE"]
    if apt is not None:
        have = {p.name for p in apt.iterdir() if (p / "archives").is_dir()} if apt.is_dir() else set()
        images = [] if have else list(APT_IMAGES)
        for release in sorted(apt_missing(jobs) - have):
            distro, codename, release_arch = release.split("-")
            image = f"{distro}:{codename}"
            if distro in APT_DISTROS and release_arch == arch and image not in images:
                images.append(image)
        if images:
            steps.append(("warm_apt_cache.py", images, "KODACT_HARBOR_APT_CACHE", apt))
    uv = where["KODACT_HARBOR_UV_CACHE"]
    if uv is not None:
        done = listed(uv / ".kodact-tasks") | listed(uv / ".kodact-no-uvx")
        tasks = sorted(unjudged_tasks(jobs) - done)
        if tasks:
            steps.append(("warm_uv_cache.py", tasks, "KODACT_HARBOR_UV_CACHE", uv))
    return steps, where


def host_arch() -> str:
    """This machine's architecture as dpkg names it, as the apt cache's folders do."""
    machine = platform.machine().lower()
    return {"x86_64": "amd64", "aarch64": "arm64"}.get(machine, machine)


def main() -> None:
    environ = os.environ
    steps, where = plan(environ, Path.home() / ".kodact-harbor", Path.cwd() / "jobs", host_arch())
    if steps and environ.get("KODACT_HARBOR_WARM", "1").strip().lower() not in OFF:
        say("Making the download caches once, for this run and the next (KODACT_HARBOR_WARM=0 skips this):")
        for script, args, var, folder in steps:
            if script == "warm_apt_cache.py" and not shutil.which("docker"):
                say("  apt: skipped, no docker on this machine")
                continue
            say(f"  {script} {' '.join(args)}".rstrip())
            cmd = [sys.executable, str(HERE / script), *args]
            if subprocess.run(cmd, env={**environ, var: str(folder)}, stdout=sys.stderr).returncode != 0:
                say(f"  {script} failed: the run goes on without what it would have made")
    for var, folder in where.items():
        given = environ.get(var, "").strip()
        # A folder given by hand is used as given; one by default only once it holds something.
        if folder is not None and (given or (folder.is_dir() and any(folder.iterdir()))):
            print(f"export {var}={shlex.quote(str(folder))}")
            say(f"cache: {folder} ({var})")
        else:
            print(f"unset {var}")


if __name__ == "__main__":
    main()
