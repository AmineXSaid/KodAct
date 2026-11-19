"""Tests for what a Harbor run makes and uses before it starts, run without Harbor or Docker.

    python3 -m pytest evals/harbor/test_caches.py

The usual command (`evals/harbor/run.sh subset ...`) is enough. Missing download caches are made
first, once, and what to make comes from what past jobs here showed: Torch for the agent, Python's
apt packages, and the tests' packages for each task whose tests judged nothing (they ran out of
time or failed to fetch). Each task's tests have a uv cache of their own, and a container gets its
task's alone: one shared cache took 17-18 minutes to copy in. Harbor's container start, agent setup and test limits get 3x; the agent's don't.
"""

import ast
import json
import subprocess
from pathlib import Path

import caches

HERE = Path(__file__).resolve().parent
HF_BLOCKED = "Using the latest cached version of the dataset since allenai/c4 couldn't be found on the Hugging Face Hub\n"


def trial(jobs: Path, name: str, reward: str | None = None, stdout: str = "", exception: str = "", apt_note: str = ""):
    """A past trial in `jobs`, with what its verifier and agent left."""
    d = jobs / "job-1" / name
    (d / "agent").mkdir(parents=True)
    (d / "result.json").write_text(json.dumps({"exception_info": {"exception_type": exception} if exception else None}))
    if reward is not None:
        (d / "verifier").mkdir()
        (d / "verifier" / "reward.txt").write_text(reward)
        (d / "verifier" / "test-stdout.txt").write_text(stdout)
    if apt_note:
        (d / "agent" / "apt-cache.txt").write_text(apt_note)


def steps(home: Path, jobs: Path, **environ) -> dict[str, list[str]]:
    planned, _ = caches.plan(environ, home, jobs, "amd64")
    return {script: args for script, args, _, _ in planned}


def test_a_first_run_makes_torch_and_pythons_apt_packages(tmp_path):
    home, jobs = tmp_path / "home", tmp_path / "jobs"
    assert steps(home, jobs) == {"warm_pip_cache.py": [], "warm_apt_cache.py": ["ubuntu:24.04", "debian:bookworm"]}
    # Made: nothing more to do.
    (home / "pip-wheelhouse").mkdir(parents=True)
    (home / "pip-wheelhouse" / "torch-2.9.0+cpu-cp312-cp312-manylinux_2_28_x86_64.whl").write_text("x")
    (home / "apt-cache" / "ubuntu-noble-amd64" / "archives").mkdir(parents=True)
    assert steps(home, jobs) == {}
    # Off: never made.
    assert steps(tmp_path / "other", jobs, KODACT_HARBOR_PIP_CACHE="off", KODACT_HARBOR_APT_CACHE="none") == {}


def test_the_tests_packages_are_made_for_the_tasks_whose_tests_judged_nothing(tmp_path):
    home, jobs = tmp_path / "home", tmp_path / "jobs"
    trial(jobs, "torch-tensor-parallelism__a1", exception="VerifierTimeoutError")
    trial(jobs, "reshard-c4-data__b2", reward="0", stdout=HF_BLOCKED)
    trial(jobs, "gpt2-codegolf__c3", reward="0", stdout="collected 1 item\n==== 1 failed in 0.07s ====\n")  # judged: a wrong answer
    trial(jobs, "fix-git__d4", reward="1", stdout=HF_BLOCKED)  # passed
    trial(jobs, "caffe-cifar-10__e5", exception="EnvironmentStartTimeoutError")  # the tests never started
    assert steps(home, jobs)["warm_uv_cache.py"] == ["reshard-c4-data", "torch-tensor-parallelism"]
    # Each task once: its own cache complete, or found to need nothing. A folder a download left
    # half made (no .kodact-done) is made again.
    (home / "uv-cache" / "tasks" / "torch-tensor-parallelism").mkdir(parents=True)
    assert steps(home, jobs)["warm_uv_cache.py"] == ["reshard-c4-data", "torch-tensor-parallelism"]
    (home / "uv-cache" / "tasks" / "torch-tensor-parallelism" / ".kodact-done").write_text("")
    (home / "uv-cache" / ".kodact-no-uvx").write_text("reshard-c4-data\n")
    assert "warm_uv_cache.py" not in steps(home, jobs)


def test_the_tasks_of_a_shared_uv_cache_get_their_own_and_the_rest_is_disk_to_free(tmp_path):
    home, jobs = tmp_path / "home", tmp_path / "jobs"
    uv = home / "uv-cache"
    (uv / "archive-v0").mkdir(parents=True)  # what the shared cache holds, for every task
    (uv / ".kodact-tasks").write_text("pytorch-model-cli\nwrite-compressor\n")
    assert steps(home, jobs)["warm_uv_cache.py"] == ["pytorch-model-cli", "write-compressor"]
    assert caches.shared_uv_leftovers(uv) == [uv / "archive-v0"]


def test_a_release_a_trial_had_no_apt_cache_for_is_made_next(tmp_path):
    home, jobs = tmp_path / "home", tmp_path / "jobs"
    (home / "apt-cache" / "ubuntu-noble-amd64" / "archives").mkdir(parents=True)
    trial(jobs, "a__1", apt_note="apt cache: none for debian-trixie-amd64 (python3 evals/harbor/warm_apt_cache.py <its image> makes one)")
    trial(jobs, "b__2", apt_note="apt cache: none for ubuntu-jammy-arm64 (...)")  # another machine's architecture
    trial(jobs, "c__3", apt_note="apt cache: none for pop-jammy-amd64 (...)")  # no Docker image of that name
    trial(jobs, "d__4", apt_note="apt cache: 144 packages for ubuntu-noble-amd64")
    assert steps(home, jobs)["warm_apt_cache.py"] == ["debian:trixie"]


def agent_function(name: str):
    """A function of kodact_agent.py, read from its source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    fn = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == name)
    scope: dict = {"Path": Path}
    exec(compile(ast.Module(body=[fn], type_ignores=[]), "kodact_agent.py", "exec"), scope)
    return scope[name]


def test_a_container_gets_its_tasks_own_uv_cache_and_no_other(tmp_path):
    uv_cache_for = agent_function("uv_cache_for")
    # The shared cache of before: gigabytes for every task, copied into none (17-18 minutes each).
    (tmp_path / "archive-v0").mkdir()
    (tmp_path / ".kodact-tasks").write_text("torch-tensor-parallelism\n")
    assert uv_cache_for("torch-tensor-parallelism", tmp_path) is None
    own = tmp_path / "tasks" / "torch-tensor-parallelism"
    own.mkdir(parents=True)
    assert uv_cache_for("torch-tensor-parallelism", tmp_path) is None, "an empty folder holds nothing"
    (own / "archive-v0").mkdir()
    assert uv_cache_for("torch-tensor-parallelism", tmp_path) == own
    assert uv_cache_for("fix-git", tmp_path) is None and uv_cache_for("", tmp_path) is None


def test_the_uv_warm_script_keeps_its_list_of_tasks_that_need_nothing():
    import tempfile

    import warm_uv_cache

    with tempfile.TemporaryDirectory() as d:
        for batch in (["b"], ["a", "b"], []):
            warm_uv_cache.record(Path(d), warm_uv_cache.NO_UVX, batch)
        assert (Path(d) / warm_uv_cache.NO_UVX).read_text().split() == ["a", "b"]


def harbor_args(**settings: str) -> list[str]:
    """What verifier-ca.sh adds to `harbor run`, with these settings."""
    env = {"PATH": "/usr/bin:/bin", **settings}
    out = subprocess.run(["bash", "-c", f'. "{HERE / "verifier-ca.sh"}"; printf "%s\\n" "${{verifier_ca[@]}}"'],
                         env=env, capture_output=True, text=True, check=True).stdout
    return out.split()


def test_harbor_gives_slow_downloads_more_time_but_not_the_agent():
    args = harbor_args()
    for flag in ("--environment-build-timeout-multiplier", "--agent-setup-timeout-multiplier",
                 "--verifier-timeout-multiplier"):
        assert args[args.index(flag) + 1] == "3", args
    assert not any(a in args for a in ("--timeout-multiplier", "--agent-timeout-multiplier")), "the agent's own time stays"
    assert harbor_args(KODACT_HARBOR_TIMEOUT_MULTIPLIER="5")[1] == "5"
    assert "--verifier-timeout-multiplier" not in harbor_args(KODACT_HARBOR_TIMEOUT_MULTIPLIER="1"), "Harbor's own"
