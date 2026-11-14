#!/usr/bin/env python3
"""Download, once, the packages an agent may `pip install`, for runs on a slow network.

    python3 evals/harbor/warm_pip_cache.py            # Torch (CPU), the usual one
    python3 evals/harbor/warm_pip_cache.py torch torchvision --python 3.11 3.12 3.13

Some tasks (pytorch-model-cli) do not give the agent a package it wants, so the agent installs it
itself with `pip install`. Through a slow proxy that download can outlast the task's time limit, or
fall back to the far larger CUDA build and never finish. This fetches the CPU wheels once into a
wheelhouse (KODACT_HARBOR_PIP_CACHE, default ~/.kodact-harbor/pip-wheelhouse). With that variable set,
every agent copies the wheelhouse into its container and pip is pointed there (PIP_FIND_LINKS), so
the agent's own `pip install torch` comes from disk; anything not cached still falls back to the
index. The task still withholds the package — only the download speed changes — and it applies to
every agent, so comparisons stay even.

pip takes only wheels for its own Python, so the wheels are fetched for each Python a task's
container may have (--python, default 3.12 and 3.13: Ubuntu 24.04's apt gives 3.12, the
python:3.13 images 3.13). With KODACT_HARBOR_CA_BUNDLE set, pip here trusts it too. The run
scripts use the wheelhouse whenever it exists; jobs that use it get -pipcache.
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path

ATTEMPTS = 3
TORCH_CPU_INDEX = "https://download.pytorch.org/whl/cpu"
# The Pythons the wheels are for: what task containers have or an agent gets from apt.
PYTHONS = ["3.12", "3.13"]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("packages", nargs="*", default=["torch"])
    parser.add_argument("--python", nargs="+", default=PYTHONS, help="the Python versions to fetch wheels for")
    args = parser.parse_args()
    pkgs = args.packages
    house = Path(os.environ.get("KODACT_HARBOR_PIP_CACHE") or Path.home() / ".kodact-harbor" / "pip-wheelhouse")
    house = house.expanduser()
    house.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("PIP_")}
    env["PIP_DEFAULT_TIMEOUT"] = "600"  # a slow proxy needs far longer than pip's default
    ca = os.environ.get("KODACT_HARBOR_CA_BUNDLE")
    if ca:
        env["PIP_CERT"] = str(Path(ca).resolve())
    # The CPU index holds the Torch wheels; its dependencies come from PyPI. Asking for the CPU
    # build keeps this to ~180 MB instead of the multi-GB CUDA build with its NVIDIA packages.
    for version in args.python:
        cmd = [
            sys.executable, "-m", "pip", "download", *pkgs,
            "--index-url", TORCH_CPU_INDEX,
            "--extra-index-url", "https://pypi.org/simple",
            "--python-version", version, "--only-binary=:all:",
            "-d", str(house),
        ]
        print(" ".join(cmd), flush=True)
        for attempt in range(1, ATTEMPTS + 1):
            if subprocess.run(cmd, env=env).returncode == 0:
                break
            print(f"attempt {attempt} of {ATTEMPTS} failed" + ("; trying again" if attempt < ATTEMPTS else ""), flush=True)
        else:
            sys.exit(f"failed: could not fill the wheelhouse for Python {version}")
    wheels = list(house.glob("*.whl"))
    size = sum(w.stat().st_size for w in wheels)
    print(f"\nwheelhouse: {house} ({len(wheels)} wheels, {size / 1e9:.2f} GB)")
    print("the run scripts use it whenever it exists")


if __name__ == "__main__":
    main()
