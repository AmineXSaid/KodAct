"""Tests for dataset.sh: which Terminal-Bench the run scripts give `harbor run -d`, the job name's
tag, and the -i/-x patterns a registry dataset needs. Run without Harbor:

    python3 -m pytest evals/harbor/test_dataset.py
"""

import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent


def run(dataset: str | None, *args: str) -> list[str]:
    """dataset.sh sourced as the run scripts do (set -euo pipefail), then task_args ARGS: the
    dataset, its tag, the exported variable and the arguments harbor gets, one per line."""
    env = {"PATH": "/usr/bin:/bin"} | ({"KODACT_HARBOR_DATASET": dataset} if dataset is not None else {})
    script = (
        'set -euo pipefail; . "$1"; shift; task_args "$@"; '
        'printf "%s\\n" "$dataset" "$dataset_tag" "$KODACT_HARBOR_DATASET" ${task_args_out[@]+"${task_args_out[@]}"}'
    )
    out = subprocess.run(
        ["bash", "-c", script, "bash", str(HERE / "dataset.sh"), *args], env=env, capture_output=True, text=True, check=True
    )
    return out.stdout.splitlines()


def test_terminal_bench_2_1_is_the_default_and_plain_task_names_get_its_namespace():
    out = run(None, "-i", "mteb-leaderboard", "-i", "terminal-bench/fix-git", "-x", "qemu-*", "-l", "3")
    assert out[:3] == ["terminal-bench/terminal-bench-2-1", "tb2.1", "terminal-bench/terminal-bench-2-1"]
    assert out[3:] == ["-i", "*/mteb-leaderboard", "-i", "terminal-bench/fix-git", "-x", "*/qemu-*", "-l", "3"]
    assert run(None, "--include-task-name=pytorch*")[3:] == ["--include-task-name=*/pytorch*"]
    assert run(None)[3:] == []


def test_terminal_bench_2_0_still_runs_with_its_patterns_unchanged():
    out = run("terminal-bench@2.0", "-i", "mteb-leaderboard")
    assert out == ["terminal-bench@2.0", "tb2.0", "terminal-bench@2.0", "-i", "mteb-leaderboard"]


def test_a_pinned_revision_keeps_the_tag_and_another_dataset_gets_its_own():
    assert run("terminal-bench/terminal-bench-2-1@6")[1] == "tb2.1"
    assert run("terminal-bench/terminal-bench")[1] == "terminal-bench-terminal-bench"
