#!/usr/bin/env python3
"""Fill a uv cache, once, with what a task's tests download, for runs on a slow network.

    python3 evals/harbor/warm_uv_cache.py pytorch-model-cli [more-tasks...]

Terminal-Bench tests install their tools with `uvx` (pytorch-model-cli: Torch,
torchvision, OpenCV, numpy, pytest) every time they run. Through a slow company
proxy that download alone can outlast the task's verifier limit, and the task
scores 0 whatever the agent did. This script reads each task's own test script,
runs its `uvx` line once on this machine (same uv version, same packages, with
the tool's `--version` instead of the tests), and keeps what uv downloaded in a
folder of the task's own: KODACT_HARBOR_UV_CACHE/tasks/<task> (by default
~/.kodact-harbor/uv-cache, which the run scripts use whenever it exists). The
agent copies that folder alone into the task's container, and the run scripts
point the tests' uv at it (UV_CACHE_DIR, see verifier-ca.sh), so the packages come
from disk. uv still asks the index whether they changed, which is small.

The task scripts come from the repository of the Terminal-Bench version the run uses
(KODACT_HARBOR_DATASET, as dataset.sh sets it: 2.1 by default, or 2.0), at the commit its
dataset was made from (KODACT_HARBOR_TB21_COMMIT, KODACT_HARBOR_TB2_COMMIT change it). A
task's cache records what it was made from; one made from another version's scripts is made
again, into the same folder, so what the two share isn't downloaded twice. With
KODACT_HARBOR_CA_BUNDLE set, uv on this machine trusts it too.
"""

import os
import re
import shlex
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

# Each version's task scripts: the repository, the variable that changes the commit, the commit
# its dataset was made from, and a task's test script in it. 2.1's tasks sit under tasks/.
SOURCES = {
    "2.0": (
        "https://raw.githubusercontent.com/laude-institute/terminal-bench-2",
        "KODACT_HARBOR_TB2_COMMIT",
        "69671fbaac6d67a7ef0dfec016cc38a64ef7a77c",
        "{task}/tests/test.sh",
    ),
    "2.1": (
        "https://raw.githubusercontent.com/harbor-framework/terminal-bench-2-1",
        "KODACT_HARBOR_TB21_COMMIT",
        "12b7565679ce5006230b820666353673a133db93",
        "tasks/{task}/tests/test.sh",
    ),
}
DEFAULT_DATASET = "terminal-bench/terminal-bench-2-1"
ATTEMPTS = 3
# uvx options that take a value; anything else starting with "-" is a flag.
WITH_VALUE = {
    "-p", "--python", "-w", "--with", "--from", "--index", "--default-index", "--index-url",
    "--extra-index-url", "--index-strategy", "--with-requirements", "--find-links", "-f",
}


def version(environ=os.environ) -> str | None:
    """The Terminal-Bench version KODACT_HARBOR_DATASET names (2.1 when unset), or None for a
    dataset whose task scripts this script doesn't know."""
    dataset = (environ.get("KODACT_HARBOR_DATASET") or DEFAULT_DATASET).strip()
    if dataset == "terminal-bench@2.0":
        return "2.0"
    if dataset.split("@", 1)[0] == "terminal-bench/terminal-bench-2-1":
        return "2.1"
    return None


def source_id(environ=os.environ) -> str:
    """What a task's cache is made from, written in its .kodact-done: the version and the commit."""
    v = version(environ) or "?"
    _, var, commit, _ = SOURCES.get(v, ("", "", "", ""))
    return f"terminal-bench {v} @ {environ.get(var) or commit}"


def is_made(marker: str, environ=os.environ) -> bool:
    """Whether a task's cache, whose .kodact-done says `marker`, was made from this version's
    scripts. An empty one predates the record, and was made from 2.0's."""
    text = marker.strip()
    return text == source_id(environ) or (text == "" and version(environ) == "2.0")


def no_uvx_list(environ=os.environ) -> str:
    """The list of tasks found to need nothing, for this version: 2.0's keeps its old name."""
    v = version(environ)
    return NO_UVX if v == "2.0" else f"{NO_UVX}-{v}"


def test_script(task: str, environ=os.environ) -> str:
    repo, var, commit, path = SOURCES[version(environ) or "2.1"]
    # A registry dataset names its tasks org/task: the folder is the task's own name.
    url = f"{repo}/{environ.get(var) or commit}/{path.format(task=task.split('/')[-1])}"
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read().decode()


def uvx_line(script: str) -> list[str] | None:
    """The script's `uvx ...` command as arguments, its backslash continuations joined."""
    joined = re.sub(r"\\\n", " ", script)
    for line in joined.splitlines():
        words = shlex.split(line, comments=True)
        if words and words[0] == "uvx":
            return words
    return None


def warm_args(words: list[str]) -> list[str]:
    """uvx's options as the test gives them, then its tool with `--version` instead of the tests."""
    out, i = [], 1
    while i < len(words):
        w = words[i]
        if w in WITH_VALUE:
            out += words[i : i + 2]
            i += 2
        elif w.startswith("-"):
            out.append(w)
            i += 1
        else:
            return out + [w, "--version"]
    raise ValueError("no tool after uvx's options")


def uv_version(script: str) -> str | None:
    """The uv version the script installs (astral.sh/uv/<version>/install.sh), if it pins one."""
    m = re.search(r"astral\.sh/uv/([0-9][0-9.]*)/install\.sh", script)
    return m.group(1) if m else None


# Each task's tests get a cache of their own, holding only what they install: TASKS/<task>, with
# DONE in it once complete. The agents copy that folder alone into the task's container
# (kodact_agent.uv_cache_for). One cache shared by every task held gigabytes: copying it into a
# container took 17-18 minutes, past Harbor's setup limit for four tasks (2026-11-17).
# NO_UVX lists the tasks found to need nothing, so caches.py doesn't ask again. LEGACY is the
# shared cache's list: caches.py makes each of its tasks a folder of its own.
TASKS = "tasks"
DONE = ".kodact-done"
NO_UVX = ".kodact-no-uvx"
LEGACY = ".kodact-tasks"


def record(cache: Path, name: str, tasks: list[str]) -> None:
    """Add `tasks` to the list `name` in the cache."""
    path = cache / name
    have = set(path.read_text().split()) if path.is_file() else set()
    if tasks:
        path.write_text("".join(f"{t}\n" for t in sorted(have | set(tasks))))


def prune_environments(cache: Path) -> int:
    """Drop what only works on this machine, keep the downloaded packages.

    uvx also caches the environments it builds, whose bin/python links to this machine's
    interpreter; docker cp copies such a link as it is, and in a container it would dangle.
    The environments are cheap to rebuild from the cached packages, so they go, with any link
    left pointing at nothing.
    """
    root = cache.resolve()
    dropped = 0
    for bucket in cache.glob("archive-v*"):
        for entry in bucket.iterdir():
            links = (p for p in entry.rglob("*") if p.is_symlink())
            if any(not p.resolve().is_relative_to(root) for p in links):
                shutil.rmtree(entry)
                dropped += 1
    for p in list(cache.rglob("*")):
        if p.is_symlink() and not p.exists():
            p.unlink()
    return dropped


def main() -> None:
    tasks = [t.split("/")[-1] for t in sys.argv[1:]]
    if not tasks:
        sys.exit(__doc__)
    if version() is None:
        print(f"no task scripts known for {os.environ.get('KODACT_HARBOR_DATASET')}: nothing to cache")
        return
    cache = Path(os.environ.get("KODACT_HARBOR_UV_CACHE") or Path.home() / ".kodact-harbor" / "uv-cache").expanduser()
    cache.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if k != "UV_CACHE_DIR"}
    # uv gives up on a download after 30 s without data; through a slow proxy, Torch and OpenCV
    # need far longer.
    env.setdefault("UV_HTTP_TIMEOUT", "600")
    ca = os.environ.get("KODACT_HARBOR_CA_BUNDLE")
    if ca:
        env["SSL_CERT_FILE"] = str(Path(ca).resolve())
    failed, no_uvx = [], []
    for task in tasks:
        # A script that can't be read (no such task in this version, the network) fails that task
        # alone: the others still get their caches.
        try:
            script = test_script(task)
        except (OSError, ValueError) as e:
            print(f"{task}: its test script could not be read ({e})", flush=True)
            failed.append(task)
            continue
        words = uvx_line(script)
        if not words:
            print(f"{task}: its tests don't use uvx, nothing to cache")
            no_uvx.append(task)
            continue
        pinned = uv_version(script)
        # The tests' own uv version, so the cache has the layout that version reads.
        # The outer uvx only fetches that uv; it keeps it in its usual cache, not this one.
        runner = ["uvx", "--from", f"uv=={pinned}", "uvx"] if pinned else ["uvx"]
        own = cache / TASKS / task
        own.mkdir(parents=True, exist_ok=True)
        cmd = runner + ["--cache-dir", str(own)] + warm_args(words)
        print(f"{task}: {shlex.join(cmd)}", flush=True)
        # A slow proxy can drop a download part way; what finished stays in the cache.
        for attempt in range(1, ATTEMPTS + 1):
            if subprocess.run(cmd, env=env).returncode == 0:
                break
            print(f"{task}: attempt {attempt} of {ATTEMPTS} failed" + ("; trying again" if attempt < ATTEMPTS else ""), flush=True)
        else:
            failed.append(task)
        dropped = prune_environments(own)
        if task not in failed:
            (own / DONE).write_text(source_id() + "\n")
        size = sum(p.stat().st_size for p in own.rglob("*") if p.is_file() and not p.is_symlink())
        print(f"{task}: {own} ({size / 1e9:.2f} GB)" + (f", {dropped} environments built here dropped" if dropped else ""))
    record(cache, no_uvx_list(), no_uvx)
    if failed:
        sys.exit(f"failed: {' '.join(failed)}")
    print("the run scripts use it whenever it exists")


if __name__ == "__main__":
    main()
