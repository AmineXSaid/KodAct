"""Tests for keeping a trial's agent off the benchmark's own files: kodact_agent.py gives kodact the
URLs to refuse (KODACT_BLOCKED_URLS), and run.sh names a job that lets them through.

    python3 -m pytest evals/harbor/test_benchmark_block.py

What they hold: by default every trial blocks the benchmark's name in any URL and its task's tests,
solution and task.toml wherever they are copied (df0446d fetched torch-pipeline-parallelism's
hidden tests and solution from a public copy of Terminal-Bench 2.1); the task comes from the trial's
folder, as report.py reads it; KODACT_HARBOR_BLOCK_BENCHMARK=0 turns it off, and the job name says so.
"""

import ast
import os
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(*names: str) -> dict:
    """The named functions and constants of kodact_agent.py, read from the source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    body = [
        n
        for n in tree.body
        if (isinstance(n, ast.FunctionDef) and n.name in names)
        or (isinstance(n, ast.Assign) and any(getattr(t, "id", None) in names for t in n.targets))
    ]
    scope: dict = {"os": os, "Path": Path}
    exec(compile(ast.Module(body=body, type_ignores=[]), "kodact_agent.py", "exec"), scope)
    return scope


def blocks(task, **settings: str) -> list[str]:
    scope = load("BENCHMARK_NAMES", "benchmark_blocks")
    saved = {k: os.environ.pop(k) for k in list(os.environ) if k.startswith("KODACT_HARBOR_")}
    try:
        os.environ.update(settings)
        return scope["benchmark_blocks"](task)
    finally:
        for k in settings:
            os.environ.pop(k, None)
        os.environ.update(saved)


def test_every_trial_blocks_the_benchmark_and_its_tasks_tests_and_solution():
    b = blocks("torch-pipeline-parallelism")
    assert "terminal-bench" in b and "tbench.ai" in b, b
    assert "*/torch-pipeline-parallelism/tests*" in b and "*/torch-pipeline-parallelism/solution*" in b, b
    # Without a task (a folder Harbor named otherwise), the benchmark's name is still blocked.
    assert blocks(None) == [x for x in b if "torch-pipeline" not in x]


def test_the_blocks_are_off_when_asked():
    for value in ("0", "false", "no", "off", "OFF"):
        assert blocks("t", KODACT_HARBOR_BLOCK_BENCHMARK=value) == [], value
    assert blocks("t", KODACT_HARBOR_BLOCK_BENCHMARK="1")


def test_the_task_comes_from_the_trial_folder():
    task = load("trial_task")["trial_task"]
    assert task(Path("jobs/job-1/torch-pipeline-parallelism__yEHUSkr/agent")) == "torch-pipeline-parallelism"
    assert task(Path("somewhere/agent")) is None
    assert task(None) is None


def test_the_patterns_catch_the_copy_that_was_fetched_and_spare_the_task_s_upstream():
    # kodact matches each entry as a glob over the URL (`*` crosses `/`), and one without `*` as a
    # substring, without regard to case: fnmatch is the same on these.
    from fnmatch import fnmatch

    def blocked(url: str) -> bool:
        return any(fnmatch(url.lower(), (p if "*" in p else f"*{p}*").lower()) for p in blocks("torch-pipeline-parallelism"))

    for url in [
        "https://raw.githubusercontent.com/AI-Harness-Evals/terminal-bench-2-1-native/main/torch-pipeline-parallelism/tests/test_outputs.py",
        "https://api.github.com/repos/AI-Harness-Evals/terminal-bench-2-1-native/contents/torch-pipeline-parallelism/solution",
        "https://gitlab.com/someone/tasks/-/raw/main/torch-pipeline-parallelism/tests/test.sh",
        "https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/legacy/mteb-retrieve/environment/task-deps/data.txt",
        "https://www.tbench.ai/leaderboard",
    ]:
        assert blocked(url), url
    for url in [
        "https://raw.githubusercontent.com/huggingface/picotron_tutorial/master/step7_pipeline_parallel_afab/pipeline_parallel.py",
        "https://pypi.org/simple/torch/",
        "https://html.duckduckgo.com/html/?q=%22train_step_pipeline_afab%22",
    ]:
        assert not blocked(url), url


def test_the_job_name_says_the_benchmark_was_open():
    run = (HERE / "run.sh").read_text()
    lines = [ln for ln in run.splitlines() if ln.startswith('case "${KODACT_HARBOR_BLOCK_BENCHMARK')]
    assert len(lines) == 1, lines
    script = 'ca=""\n' + lines[0] + '\nprintf "%s" "$ca"'
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_")}
    for value, name in [("0", "-benchopen"), ("off", "-benchopen"), ("1", ""), ("", "")]:
        r = subprocess.run(["bash", "-c", script], env={**env, "KODACT_HARBOR_BLOCK_BENCHMARK": value}, capture_output=True, text=True)
        assert r.stdout == name, (value, r.stdout)
    r = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
    assert r.stdout == "", r.stdout
