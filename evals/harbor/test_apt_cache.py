"""Tests for the apt cache every agent copies into its container (upload_apt_cache), run without
Harbor or Docker.

    python3 -m pytest evals/harbor/test_apt_cache.py

When a task's container has no Python, the agent installs it with apt, which through a slow proxy
took 10 of the task's 14 minutes. warm_apt_cache.py downloads those packages once per release; in
the container, apt's downloads are pointed at them, out of reach of the Docker images' docker-clean
hook, which empties /var/cache/apt/archives after every `apt-get update`. The package lists go in
only when the image has none. Checked on real apt (Ubuntu 24.04, 2026-11-14): after `apt-get
update`, `apt-get install --download-only python3-venv python3-numpy` needed 0 B of 4438 kB.
"""

import ast
import re
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent


def agent_parts() -> dict:
    """apt_release, use_apt_cache and their constants, from kodact_agent's source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    names = ("APT_RELEASE", "USE_APT_CACHE", "KEPT_APT", "KEPT_APT_LISTS", "APT_CONF")
    scope: dict = {"re": re}
    for node in tree.body:
        if isinstance(node, ast.Assign) and getattr(node.targets[0], "id", None) in names:
            scope[node.targets[0].id] = ast.literal_eval(node.value)
    fns = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in ("apt_release", "use_apt_cache")]
    exec(compile(ast.Module(body=fns, type_ignores=[]), "kodact_agent.py", "exec"), scope)
    return scope


def run_use(tmp: Path, has_lists: bool) -> tuple[str, Path, Path]:
    """USE_APT_CACHE on temporary folders, as uploaded: (what it said, apt.conf line file, apt lists)."""
    parts = agent_parts()
    archives, staged, apt_lists = tmp / "archives", tmp / "staged-lists", tmp / "var-lib-apt-lists"
    archives.mkdir()
    (archives / "python3-venv_3.12.3_amd64.deb").write_text("deb")
    staged.mkdir()
    (staged / "archive.ubuntu.com_ubuntu_dists_noble_main_binary-amd64_Packages.lz4").write_text("cached")
    apt_lists.mkdir()
    if has_lists:
        (apt_lists / "mirror_dists_noble_main_binary-amd64_Packages.lz4").write_text("image's own")
    conf = tmp / "00kodact-apt-cache"
    script = parts["use_apt_cache"]("ubuntu-noble-amd64", conf=str(conf), apt_lists=str(apt_lists))
    script = script.replace(parts["KEPT_APT"], str(archives)).replace(parts["KEPT_APT_LISTS"], str(staged))
    # As Harbor runs it: under pipefail.
    out = subprocess.run(["bash", "-c", f"set -o pipefail; {script}"], capture_output=True, text=True)
    assert out.returncode == 0, out.stderr
    return out.stdout.strip(), conf, apt_lists


def test_apts_downloads_go_where_the_cache_is(tmp_path):
    said, conf, apt_lists = run_use(tmp_path, has_lists=False)
    assert conf.read_text() == f'Dir::Cache::Archives "{tmp_path / "archives"}/";\n'
    assert (tmp_path / "archives" / "partial").is_dir(), "apt downloads what's missing into partial/"
    # An image without lists (most delete theirs) gets the cached ones: apt knows the cached versions.
    assert [p.name for p in apt_lists.iterdir()] == ["archive.ubuntu.com_ubuntu_dists_noble_main_binary-amd64_Packages.lz4"]
    assert said == "apt cache: 1 packages for ubuntu-noble-amd64 and its package lists", said
    assert not (tmp_path / "staged-lists").exists(), "the staged copy is removed"


def test_an_images_own_package_lists_are_kept(tmp_path):
    said, _, apt_lists = run_use(tmp_path, has_lists=True)
    assert [p.read_text() for p in apt_lists.iterdir()] == ["image's own"]
    assert said == "apt cache: 1 packages for ubuntu-noble-amd64", said


def test_the_release_names_a_folder_or_nothing():
    release = agent_parts()["apt_release"]
    assert release("ubuntu-noble-amd64\n") == "ubuntu-noble-amd64"
    assert release("noise\ndebian-bookworm-arm64\n") == "debian-bookworm-arm64"
    # No apt, no codename, or anything that isn't a plain name (it becomes a path on this machine).
    for out in (None, "", "debian--amd64", "../../etc-x-y", "ubuntu-noble-amd64; rm -rf /"):
        assert release(out) == "", out
    # Read here as the container reads itself, when this machine has apt.
    here = subprocess.run(["bash", "-c", agent_parts()["APT_RELEASE"]], capture_output=True, text=True).stdout
    assert here == "" or release(here), here


def test_the_warm_script_takes_package_names_only():
    out = subprocess.run(["python3", str(HERE / "warm_apt_cache.py"), "--packages", "python3", "x;rm -rf /"],
                         capture_output=True, text=True)
    assert out.returncode != 0 and "not a package name: x;rm -rf /" in out.stderr, out.stderr


def settled(home: Path, **settings: str) -> dict:
    """The cache settings after caches.sh, as the run scripts source it, with `home` as HOME."""
    script = f'. "{HERE / "caches.sh"}"; for v in UV PIP APT; do n=KODACT_HARBOR_${{v}}_CACHE; printf "%s=%s\\n" "$n" "${{!n-unset}}"; done'
    env = {"HOME": str(home), "PATH": "/usr/bin:/bin", **settings}
    out = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, check=True).stdout
    return dict(line.split("=", 1) for line in out.splitlines() if "=" in line and not line.startswith("cache:"))


def test_the_caches_are_used_whenever_their_folders_exist(tmp_path):
    caches = tmp_path / ".kodact-harbor"
    assert set(settled(tmp_path).values()) == {"unset"}, "no folder, no cache"
    for name in ("uv-cache", "pip-wheelhouse", "apt-cache"):
        (caches / name).mkdir(parents=True)
    assert settled(tmp_path) == {
        "KODACT_HARBOR_UV_CACHE": str(caches / "uv-cache"),
        "KODACT_HARBOR_PIP_CACHE": str(caches / "pip-wheelhouse"),
        "KODACT_HARBOR_APT_CACHE": str(caches / "apt-cache"),
    }
    # Off turns one off; a path picks another folder.
    got = settled(tmp_path, KODACT_HARBOR_APT_CACHE="off", KODACT_HARBOR_UV_CACHE="/elsewhere")
    assert got["KODACT_HARBOR_APT_CACHE"] == "unset" and got["KODACT_HARBOR_UV_CACHE"] == "/elsewhere", got


def test_every_run_script_settles_the_caches_and_names_them_in_the_job():
    for name in ("run.sh", "run-pi.sh", "run-ante.sh", "run-forgecode.sh"):
        script = (HERE / name).read_text()
        assert script.index("caches.sh") < script.index('-uvcache"') < script.index('-aptcache"'), name
