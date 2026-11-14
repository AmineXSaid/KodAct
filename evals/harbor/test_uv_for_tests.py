"""Tests for the uv kodact_agent gives a task's tests (provide_uv), run without Harbor or Docker.

    python3 -m pytest evals/harbor/test_uv_for_tests.py

Terminal-Bench test scripts download uv themselves; when that fails ("Could not resolve host:
releases.astral.sh"), `uvx: command not found` and nothing is judged. After the agent's run, the
agent puts this machine's uv where the installer would: then `source $HOME/.local/bin/env` and
`uvx` work as after an install. A uv already there is left alone, and one that doesn't run in
the container (another libc or architecture) is taken away again. The script runs here on a
temporary HOME, with this machine's uv.
"""

import ast
import os
import shutil
import subprocess
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
UV = shutil.which("uv")


def constant(name: str) -> str:
    """A string constant of kodact_agent.py, read from the source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", None) == name for t in node.targets):
            return ast.literal_eval(node.value)
    raise KeyError(name)


def provide(home: Path, src: Path) -> str:
    """Run UV_FOR_TESTS as kodact_agent does, on `home`, with `src` uploaded; what it said."""
    script = constant("UV_FOR_TESTS").replace("@SRC@", str(src))
    env = {"HOME": str(home), "PATH": "/usr/bin:/bin"}  # no uvx on PATH, as in a fresh container
    return subprocess.run(["sh", "-c", f"{{ {script} }} 2>&1"], env=env, capture_output=True, text=True).stdout


@pytest.mark.skipif(UV is None, reason="no uv on this machine")
def test_the_tests_find_uv_where_its_installer_puts_it(tmp_path):
    home, src = tmp_path / "root", tmp_path / "kodact-uv-for-tests"
    home.mkdir()
    shutil.copy(UV, src)
    said = provide(home, src)
    assert said.startswith("uv for the tests: uv ") and "from the machine running Harbor" in said, said
    assert not src.exists(), "the upload is cleaned up"
    # What a test script does next: source the env file, then call uvx.
    test_sh = 'source "$HOME/.local/bin/env" && command -v uvx && uvx --version && uv tool run --help >/dev/null'
    out = subprocess.run(["bash", "-c", test_sh], env={"HOME": str(home), "PATH": "/usr/bin:/bin"},
                         capture_output=True, text=True)
    assert out.returncode == 0, out.stderr
    assert out.stdout.splitlines()[0] == f"{home}/.local/bin/uvx", out.stdout
    assert out.stdout.splitlines()[1].startswith("uv "), out.stdout


def test_a_uv_already_there_is_left_alone(tmp_path):
    home, src = tmp_path / "root", tmp_path / "kodact-uv-for-tests"
    (home / ".local" / "bin").mkdir(parents=True)
    (home / ".local" / "bin" / "uv").write_text("#!/bin/sh\necho mine\n")
    (home / ".local" / "bin" / "uv").chmod(0o755)
    src.write_text("other")
    assert provide(home, src).strip() == "uv for the tests: one is there already"
    assert (home / ".local" / "bin" / "uv").read_text() == "#!/bin/sh\necho mine\n"
    assert not (home / ".local" / "bin" / "env").exists() and not src.exists()


def test_a_uv_that_does_not_run_here_is_taken_away(tmp_path):
    home, src = tmp_path / "root", tmp_path / "kodact-uv-for-tests"
    home.mkdir()
    src.write_bytes(b"\x7fELF not for this machine")
    assert "doesn't run here" in provide(home, src)
    assert sorted(os.listdir(home / ".local" / "bin")) == [], "nothing half made: the tests fetch uv themselves"
