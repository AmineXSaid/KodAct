#!/usr/bin/env python3
"""Download, once, the Debian packages an agent installs when a task's container has no Python.

    python3 evals/harbor/warm_apt_cache.py                     # ubuntu:24.04 and debian:bookworm
    python3 evals/harbor/warm_apt_cache.py ubuntu:22.04 --packages python3 python3-pip git

Some tasks give the agent no Python (torch-tensor-parallelism's Ubuntu 24.04 has none), so the
agent runs `apt-get update && apt-get install -y python3 python3-pip`. Through a slow proxy that
took 10 of the task's 14 minutes (2026-11-14), leaving no time to test anything. This runs each
image once in Docker, downloads those packages there (with what they recommend, as the agent's
`apt-get install -y` gets: a compiler and the Python headers), and keeps the downloaded .deb files
and the package lists in KODACT_HARBOR_APT_CACHE (default ~/.kodact-harbor/apt-cache), one
folder per release: ubuntu-noble-amd64/, debian-bookworm-amd64/.

Every agent then copies the folder for its container's release in and points apt's downloads at
it (kodact_agent.upload_apt_cache), so the agent's own install takes the files from disk. The run
scripts use the folder whenever it exists. The task still withholds Python: the agent still
installs it itself, and only the download speed changes, for every agent alike. A package the
mirror has updated since is downloaded as before; run this again now and then.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

IMAGES = ["ubuntu:24.04", "debian:bookworm"]
# What agents install when there is no Python; pip's recommends bring the compiler and headers.
PACKAGES = ["python3", "python3-pip", "python3-venv"]
OFF = ("none", "off", "0", "no", "false")

# Runs inside each image. The release names the folder as kodact_agent.APT_RELEASE reads it.
SCRIPT = r"""
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --download-only $PACKAGES >/dev/null
. /etc/os-release
release="$ID-$VERSION_CODENAME-$(dpkg --print-architecture)"
out="/out/$release"
rm -rf "$out"
mkdir -p "$out/archives" "$out/lists"
for f in /var/cache/apt/archives/*.deb; do [ -e "$f" ] && cp "$f" "$out/archives/"; done
cp -a /var/lib/apt/lists/. "$out/lists/"
rm -rf "$out/lists/partial" "$out/lists/lock" "$out/lists/auxfiles"
chown -R "$OWNER" "$out"
echo "$release: $(ls "$out/archives" | wc -l) packages, $(du -sh "$out" | cut -f1)"
"""


def cache_root() -> Path:
    value = os.environ.get("KODACT_HARBOR_APT_CACHE", "").strip()
    if not value or value.lower() in OFF:
        return Path.home() / ".kodact-harbor" / "apt-cache"
    return Path(value).expanduser()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("images", nargs="*", default=IMAGES, help="Docker images, one per release")
    parser.add_argument("--packages", nargs="+", default=PACKAGES, help="what to download")
    args = parser.parse_args()
    bad = [p for p in args.packages if not re.fullmatch(r"[a-z0-9][a-z0-9+.:=~-]*", p)]
    if bad:
        sys.exit(f"not a package name: {' '.join(bad)}")
    root = cache_root()
    root.mkdir(parents=True, exist_ok=True)
    owner = f"{os.getuid()}:{os.getgid()}"
    for image in args.images:
        print(f"{image}: downloading {' '.join(args.packages)}", flush=True)
        cmd = ["docker", "run", "--rm", "-v", f"{root.resolve()}:/out",
               "-e", f"PACKAGES={' '.join(args.packages)}", "-e", f"OWNER={owner}", image, "sh", "-c", SCRIPT]
        if subprocess.run(cmd).returncode != 0:
            sys.exit(f"{image}: failed")
    print(f"\napt cache: {root} (the run scripts use it whenever it exists)")


if __name__ == "__main__":
    main()
