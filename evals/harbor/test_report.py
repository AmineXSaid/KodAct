"""Tests for report.py's verdicts, run without Harbor, Docker or an endpoint.

    python3 -m pytest evals/harbor/test_report.py

The one that matters here: a trial where the verifier's own test phase could not
fetch its packages or data (a cert/network failure behind a TLS-inspecting proxy)
is set apart as "verifier error", not counted as the agent failing, while a trial
whose tests actually ran and failed stays "false success". The fixtures are cut
down from real terminal-bench 2.0 verifier logs (pytorch-model-cli, write-compressor,
gpt2-codegolf) on the KPIT endpoint.
"""

import json
from pathlib import Path

import report


def make_trial(
    tmp: Path, task: str, reward: str, verifier_stdout: str, *, ran_tests: int | None, tests: list[dict] | None = None
) -> report.Trial:
    """A minimal trial dir: the agent said done (a kodact result), the verifier wrote `reward`.
    `tests` are json-ctrf's per-test records, when the test needs them."""
    d = tmp / f"{task}__TEST"
    (d / "agent").mkdir(parents=True)
    (d / "verifier").mkdir(parents=True)
    # KodAct's stream with a clean end: this is the "said done" case a false success comes from.
    result = {"type": "result", "subtype": "success", "stop_reason": "end_turn", "num_turns": 3}
    (d / "agent" / "kodact.jsonl").write_text(json.dumps(result) + "\n")
    (d / "verifier" / "reward.txt").write_text(reward)
    (d / "verifier" / "test-stdout.txt").write_text(verifier_stdout)
    if ran_tests is not None:  # a ctrf.json as json-ctrf would write it
        summary = {"tests": ran_tests, "passed": 0, "failed": ran_tests, "skipped": 0}
        records = tests or []
        (d / "verifier" / "ctrf.json").write_text(json.dumps({"results": {"summary": summary, "tests": records}}))
    return report.Trial(d)


# A verifier that died fetching its own dependencies: uv could not reach download.pytorch.org
# because the proxy's CA is not trusted in the test container, so no test body ran.
PYTORCH_INFRA = """\
Hit:1 http://deb.debian.org/debian bookworm InRelease
Reading package lists...
/tests/test.sh: line 9: /root/.local/bin/env: No such file or directory
error: Failed to fetch: `https://download-r2.pytorch.org/whl/cpu/torchvision-0.22.1%2Bcpu-cp313-cp313-manylinux_2_28_x86_64.whl.metadata`
  Caused by: Request failed after 3 retries
  Caused by: error sending request for url (https://download-r2.pytorch.org/whl/cpu/torchvision-0.22.1%2Bcpu...whl.metadata)
  Caused by: invalid peer certificate: UnknownIssuer
"""

# A verifier whose setup printed a transient cert warning, but then the tests ran and the agent's
# output genuinely failed them (pytest reached the test bodies: "3 failed").
WRITE_COMPRESSOR_REAL_FAIL = """\
Setting up ca-certificates (20260601~24.04.1) ...
curl: (60) SSL certificate problem: unable to get local issuer certificate
Installed 6 packages in 19ms
============================= test session starts ==============================
collected 3 items
../tests/test_outputs.py FFF                                             [100%]
=========================== short test summary info ============================
FAILED ../tests/test_outputs.py::test_compressed_file_exists - AssertionError
============================== 3 failed in 0.22s ===============================
"""

# A plain wrong answer: no network trouble at all, tests ran, one failed.
GPT2_REAL_FAIL = """\
============================= test session starts ==============================
collected 1 item
../tests/test_outputs.py F                                               [100%]
FAILED ../tests/test_outputs.py::test_gpt2_implementation - AssertionError
============================== 1 failed in 0.07s ===============================
"""


def test_pytorch_verifier_cert_failure_is_verifier_error(tmp_path):
    t = make_trial(tmp_path, "pytorch-model-cli", "0.0", PYTORCH_INFRA, ran_tests=None)
    assert t.verdict == "verifier error"
    assert t.verifier_infra is True


def test_tests_that_ran_and_failed_stay_false_success(tmp_path):
    # Setup had a cert hiccup, but the tests reached their bodies: this is the agent's failure.
    t = make_trial(tmp_path, "write-compressor", "0.0", WRITE_COMPRESSOR_REAL_FAIL, ran_tests=3)
    assert t.verdict == "false success"
    assert t.verifier_infra is False


def test_plain_wrong_answer_is_false_success(tmp_path):
    t = make_trial(tmp_path, "gpt2-codegolf", "0.0", GPT2_REAL_FAIL, ran_tests=1)
    assert t.verdict == "false success"


def test_verifier_error_is_left_out_of_valid_trials(tmp_path):
    trials = [
        make_trial(tmp_path / "a", "pytorch-model-cli", "0.0", PYTORCH_INFRA, ran_tests=None),
        make_trial(tmp_path / "b", "gpt2-codegolf", "0.0", GPT2_REAL_FAIL, ran_tests=1),
    ]
    invalid = ("api error", "verifier error")
    valid = [t for t in trials if not t.verdict.startswith(invalid)]
    # Only the genuine failure is a valid trial; the cert-failed verifier is set apart.
    assert [t.task for t in valid] == ["gpt2-codegolf"]


# A test that downloads its own data in its body (pytorch-model-cli fetches MNIST there): the
# tests ran ("1 failed, 5 passed"), and the one that failed never got past the download.
MNIST_DOWNLOAD_FAILED = """\
../tests/test_outputs.py .....F                                          [100%]
FAILED ../tests/test_outputs.py::test_cli_tool_output - RuntimeError: Error d...
========================= 1 failed, 5 passed in 41.12s =========================
"""
MNIST_TRACE = """\
    def test_cli_tool_output():
        # Now first download the first 50 images of the MNIST dataset
>       dataset = datasets.MNIST(root="./data", train=False, download=True)
E               RuntimeError: Error downloading train-labels-idx1-ubyte.gz:
E               Tried https://ossci-datasets.s3.amazonaws.com/mnist/, got:
E               <urlopen error [Errno -2] Name or service not known>
E               Tried http://yann.lecun.com/exdb/mnist/, got:
E               HTTP Error 403: Forbidden
"""
PREDICTIONS_TRACE = """\
    def test_cli_tool_output():
        # Now first download the first 50 images of the MNIST dataset
>       assert all_correct, "Predictions do not match expected values"
E       AssertionError: Predictions do not match expected values
"""


def ctrf_test(name: str, status: str, trace: str = "") -> dict:
    return {"name": name, "status": status, "message": "The test failed in the call phase", "trace": trace}


def test_a_test_that_failed_fetching_its_own_data_is_verifier_error(tmp_path):
    tests = [ctrf_test(f"test_{i}", "passed") for i in range(5)]
    tests.append(ctrf_test("test_cli_tool_output", "failed", MNIST_TRACE))
    t = make_trial(tmp_path, "pytorch-model-cli", "0.0", MNIST_DOWNLOAD_FAILED, ran_tests=6, tests=tests)
    assert t.verdict == "verifier error"


def test_a_wrong_answer_beside_a_network_failure_stays_the_agents(tmp_path):
    # One test judged the agent's output and failed it: the run measured something.
    tests = [
        ctrf_test("test_cli_tool_output", "failed", PREDICTIONS_TRACE),
        ctrf_test("test_download_sample", "failed", MNIST_TRACE),
    ]
    stdout = MNIST_DOWNLOAD_FAILED.replace("1 failed, 5 passed", "2 failed, 4 passed")
    t = make_trial(tmp_path, "pytorch-model-cli", "0.0", stdout, ran_tests=6, tests=tests)
    assert t.verdict == "false success"
    # The test's own source mentions downloading; only its error lines are read.
    t = make_trial(tmp_path / "b", "pytorch-model-cli", "0.0", stdout, ran_tests=6, tests=tests[:1])
    assert t.verdict == "false success"


# pytorch-model-cli (a889576): the agent stopped `apt-get install` mid-run, the tests' own
# `apt-get install curl` then refused to run, and no test started. Reward 0 was read as a false
# success; it is a failure with its own cause, and still a valid trial.
DPKG_INTERRUPTED = """\
Hit:1 http://deb.debian.org/debian bookworm InRelease
Reading package lists...
E: dpkg was interrupted, you must manually run 'dpkg --configure -a' to correct the problem.
/tests/test.sh: line 8: curl: command not found
/tests/test.sh: line 9: /root/.local/bin/env: No such file or directory
/tests/test.sh: line 18: uvx: command not found
"""


def test_tests_that_could_not_start_after_an_interrupted_dpkg_are_the_agents_failure(tmp_path):
    t = make_trial(tmp_path, "pytorch-model-cli", "0", DPKG_INTERRUPTED, ran_tests=None)
    assert t.verdict == "left dpkg interrupted"
    assert t.verifier_infra is False
    assert not t.verdict.startswith(("api error", "verifier error")), "it counts as a valid trial"


def test_test_tools_that_were_not_there_are_a_verifier_error(tmp_path):
    stdout = "Reading package lists...\n/tests/test.sh: line 18: uvx: command not found\n"
    t = make_trial(tmp_path, "pytorch-model-cli", "0", stdout, ran_tests=None)
    assert t.verdict == "verifier error"


def test_verifier_timeout_is_verifier_error(tmp_path):
    # The tests ran out of time (e.g. downloading Torch through a slow proxy) and wrote no reward:
    # nothing about the agent was measured.
    t = make_trial(tmp_path, "pytorch-model-cli", "", "", ran_tests=None)
    t.exception = "VerifierTimeoutError"
    assert t.decide() == "verifier error"


def make_job(tmp: Path, trials: list[tuple[str, float, int, list[dict]]]) -> report.Job:
    """A job dir: per trial its task, reward, agent seconds and the KodAct system events it emitted."""
    job = tmp / "kodact-subset-test"
    job.mkdir(parents=True)
    (job / "config.json").write_text(json.dumps({"agents": [{"name": "kodact_agent", "model_name": "m"}]}))
    for task, reward, secs, events in trials:
        d = job / f"{task}__T"
        (d / "agent").mkdir(parents=True)
        (d / "verifier").mkdir(parents=True)
        span = {"started_at": "2026-10-31T10:00:00Z", "finished_at": f"2026-10-31T10:{secs // 60:02d}:{secs % 60:02d}Z"}
        result = {"task_name": task, "verifier_result": {"rewards": {"reward": reward}}, "agent_execution": span}
        (d / "result.json").write_text(json.dumps(result))
        end = {"type": "result", "subtype": "success", "stop_reason": "end_turn", "num_turns": 9}
        lines = [{"type": "system", **e} for e in events] + [end]
        (d / "agent" / "kodact.jsonl").write_text("\n".join(json.dumps(x) for x in lines) + "\n")
    return report.Job(job)


def test_kodact_reminders_are_counted_per_trial_and_job(tmp_path):
    # Shaped like corewars 231fd6b: a verification reminder, and loop warnings by kind, among other
    # harness events that are not reminders (api_retry, compact_boundary).
    events = [
        {"subtype": "verification", "kind": "unchecked", "files": ["my_warrior.red"], "shell": False},
        {"subtype": "loop_guard", "kind": "no_progress", "tool": "Read", "count": 3},
        {"subtype": "loop_guard", "kind": "repeated_failure", "tool": "Bash", "count": 3, "after_changes": True},
        {"subtype": "api_retry", "attempt": 1, "error": "network error"},
        {"subtype": "compact_boundary"},
    ]
    job = make_job(tmp_path, [("winning-avg-corewars", 0.0, 1800, events), ("fix-git", 1.0, 600, [])])
    corewars = job.trials[1] if job.trials[0].task == "fix-git" else job.trials[0]
    assert dict(corewars.reminders) == {
        "verification:unchecked": 1,
        "loop_guard:no_progress": 1,
        "loop_guard:repeated_failure after changes": 1,
    }
    assert corewars.reminder_cell() == "loop_guard 2, verification 1"
    assert "| loop_guard 2, verification 1 |" in corewars.row()
    text = job.report()
    assert "- KodAct reminders: 3 in 1 of 2 trials" in text
    assert "loop_guard:repeated_failure after changes ×1" in text
    # Per job: 3 reminders over 2 trials; 40 minutes of agent time for the 1 task solved.
    cmp = report.comparison([job])
    assert "| 40m00s | 1.5 (loop_guard 1.0, verification 0.5) |" in cmp, cmp


def test_compactions_and_failed_summaries_are_counted(tmp_path):
    # Shaped like path-tracing-reverse 9ef0955: seven empty summaries before one compaction worked,
    # then a second compaction; the other trial never compacted.
    failed = {"subtype": "compact_failed", "trigger": "auto", "reason": "empty_summary", "stop_reason": "max_tokens"}
    events = [failed] * 7 + [{"subtype": "compact_boundary"}] * 2
    job = make_job(tmp_path, [("path-tracing-reverse", 0.0, 1800, events), ("fix-git", 1.0, 600, [])])
    trial = next(t for t in job.trials if t.task == "path-tracing-reverse")
    assert trial.compactions() == (2, 7, 0)
    assert "| 2/7/0 |" in trial.row()
    other = next(t for t in job.trials if t.task == "fix-git")
    assert other.compaction_cell() == ""
    assert "compactions (done/failed/note)" in job.report()
    assert "| 1.0/3.5 |" in report.comparison([job])


def test_a_run_with_no_answer_is_its_own_verdict(tmp_path):
    # Shaped like write-compressor 9ef0955: the last replies were all reasoning, so KodAct ends the
    # run as an error instead of a success; it is told apart from a false success and a crash.
    job = make_job(tmp_path, [("write-compressor", 0.0, 891, [])])
    trial = job.trials[0]
    end = {
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": True,
        "stop_reason": "max_tokens",
        "num_turns": 5,
        "errors": ["The model stopped without an answer: its last replies had only reasoning, with no text and no tool call."],
    }
    (trial.path / "agent" / "kodact.jsonl").write_text(json.dumps(end) + "\n")
    assert report.Trial(trial.path).verdict == "no answer"


def test_a_run_kodact_stopped_at_its_time_limit_is_a_timeout(tmp_path):
    # KodAct now stops at --max-time itself and writes a result, instead of being killed by
    # Harbor at the limit; either way the trial ran out of time.
    job = make_job(tmp_path, [("write-compressor", 0.0, 870, [])])
    trial = job.trials[0]
    end = {
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": True,
        "stop_reason": "max_time",
        "num_turns": 6,
        "errors": ["Reached the time limit (--max-time): the run stopped; the work done so far is left as it is."],
    }
    (trial.path / "agent" / "kodact.jsonl").write_text(json.dumps(end) + "\n")
    assert report.Trial(trial.path).verdict == "timeout"


def test_valgrind_that_cannot_start_in_the_verifier_is_a_verifier_error(tmp_path):
    # custom-memory-heap-crash (090446d): the tests ran, but valgrind aborted under Docker's file
    # descriptor limit, so the leak test judged nothing.
    stdout = (
        "--2766:0:libcfile Valgrind: FATAL: Private file creation failed.\n"
        "FAILED ../tests/test_outputs.py::test_no_memory_leaks_with_valgrind\n"
        "========================= 1 failed, 5 passed in 5.03s ==========================\n"
    )
    trial = make_trial(tmp_path, "custom-memory-heap-crash", "0", stdout, ran_tests=6)
    assert trial.verdict == "verifier error"



def test_the_report_names_the_model_the_endpoint_served(tmp_path):
    job = make_job(tmp_path, [("fix-git", 1.0, 600, [])])
    meta = {"harness": "kodact", "model": "deep-thinking", "served_model": "deepseek-v4-flash", "served_window": "524288"}
    (job.path / "fix-git__T" / "agent" / "run.json").write_text(json.dumps(meta))
    job = report.Job(job.path)
    text = job.report()
    assert "served by `deepseek-v4-flash` (window 524288 tokens)" in text, text[:600]
