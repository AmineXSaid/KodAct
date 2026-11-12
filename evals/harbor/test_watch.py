"""Tests for watch.py's chat view, run without Harbor, Docker or an endpoint.

    python3 -m pytest evals/harbor/test_watch.py

What they hold: a job is shown from its start without a flag; each step shows its
reasoning (start and end), the whole command, the start of a file written, the lines
an edit changes and the output of each call (first and last lines); KodAct's
reminders and events are named; a run's error is shown once; and while the agent
is at work with nothing new, a line says what it is waiting for. The events are cut
down from real KodAct stream-json (terminal-bench 2.0 trials).
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

import watch

HERE = Path(__file__).resolve().parent


@pytest.fixture(autouse=True)
def plain(monkeypatch):
    monkeypatch.setenv("COLUMNS", "100")
    for flag in ("full", "thinking", "compact"):
        monkeypatch.setattr(watch.OPT, flag, False)


def text(lines: list[str]) -> str:
    return "\n".join(watch.ANSI.sub("", line) for line in lines)


def assistant(*content, out=120, context=(125, 8000)) -> dict:
    usage = {"output_tokens": out, "input_tokens": context[0], "cache_read_input_tokens": context[1]}
    return {"type": "assistant", "message": {"content": list(content), "usage": usage, "stop_reason": "tool_use"}}


def result(tool_id: str, body: str, meta: dict | None = None, error: bool = False) -> dict:
    block = {"type": "tool_result", "tool_use_id": tool_id, "content": body}
    if error:
        block["is_error"] = True
    event = {"type": "user", "message": {"content": [block]}}
    if meta is not None:
        event["tool_use_result"] = meta
    return event


def test_a_step_shows_its_reasoning_its_whole_command_and_the_output():
    chat = watch.Chat()
    reasoning = "\n".join(f"thought {i}" for i in range(1, 41))
    command = "cd /app && python3 - <<'EOF'\nimport struct\nprint(1)\nEOF"
    shown = text(chat.render(assistant(
        {"type": "thinking", "thinking": reasoning},
        {"type": "text", "text": "I'll inspect the data first."},
        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": command, "description": "Inspect"}},
    )))
    assert "▸ step 1 · 120 tokens out · context 8k" in shown
    assert "∴ thought 1" in shown and "thought 4" in shown and "thought 40" in shown, "start and end of the reasoning"
    assert "thought 20" not in shown and "34 more lines of reasoning" in shown
    assert "❖ KodAct\n  I'll inspect the data first." in shown
    assert "→ $ cd /app && python3 - <<'EOF'" in shown and "    import struct" in shown and "    EOF" in shown
    assert "# Inspect" in shown
    output = "\n".join(f"line {i}" for i in range(1, 101))
    shown = text(chat.render(result("t1", output, {"stdout": output, "exitCode": 0, "interrupted": False})))
    assert "✓ exit 0 · 100 lines" in shown
    assert "line 1" in shown and "line 8" in shown and "line 100" in shown, "first and last lines of the output"
    assert "line 50" not in shown and "88 more lines" in shown


def test_a_failed_call_shows_why_and_a_reply_cut_off_says_so():
    chat = watch.Chat()
    chat.render(assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "python3 x.py"}}))
    shown = text(chat.render(result("t1", "Exit code 127\n/bin/bash: line 1: python3: command not found", {"exitCode": 127}, error=True)))
    assert "✗ exit 127" in shown and "python3: command not found" in shown
    cut_off = assistant({"type": "thinking", "thinking": "a long thought"}, out=32000)
    cut_off["message"]["stop_reason"] = "max_tokens"
    shown = text(chat.render(cut_off))
    assert "▸ step 2 · 32,000 tokens out" in shown
    assert "cut off at the output limit before any answer or action" in shown


def test_a_write_shows_the_start_of_the_file_and_an_edit_the_lines_it_changes():
    chat = watch.Chat()
    body = "\n".join(f"row {i}" for i in range(1, 31))
    shown = text(chat.render(assistant(
        {"type": "tool_use", "id": "w", "name": "Write", "input": {"file_path": "/app/gen.py", "content": body}},
        {"type": "tool_use", "id": "e", "name": "Edit", "input": {"file_path": "/app/x.py", "old_string": "return 1", "new_string": "return 2"}},
    )))
    assert "→ Write /app/gen.py · 30 lines" in shown
    assert "   1 │ row 1" in shown and "  10 │ row 10" in shown and "row 11" not in shown and "20 more lines" in shown
    assert "→ Edit /app/x.py\n    - return 1\n    + return 2" in shown
    # Two calls in one step: each result says which call it answers.
    shown = text(chat.render(result("w", "File created successfully at: /app/gen.py", {"type": "create", "filePath": "/app/gen.py"})))
    assert "✓ created ← Write /app/gen.py (30 lines)" in shown


def test_full_shows_everything():
    watch.OPT.full = True
    chat = watch.Chat()
    reasoning = "\n".join(f"thought {i}" for i in range(1, 41))
    body = "\n".join(f"row {i}" for i in range(1, 31))
    shown = text(chat.render(assistant(
        {"type": "thinking", "thinking": reasoning},
        {"type": "tool_use", "id": "w", "name": "Write", "input": {"file_path": "/app/gen.py", "content": body}},
    )))
    assert "thought 20" in shown and "row 30" in shown and "more lines" not in shown


def test_kodacts_reminders_and_events_are_named():
    chat = watch.Chat()
    reminder = {"type": "user", "isSynthetic": True, "message": {"content": [{"type": "text", "text": (
        "<system-reminder>\nYou have about 2 minutes of the run's time left (--max-time). Wrap up.\n</system-reminder>")}]}}
    assert text(chat.render(reminder)).startswith("⚑ You have about 2 minutes of the run's time left")
    stopped = {"type": "user", "isSynthetic": True, "message": {"content": [
        {"type": "text", "text": "[The run reached its time limit (--max-time) and stopped here]"}]}}
    assert text(chat.render(stopped)) == "⏱ The run reached its time limit (--max-time) and stopped here"
    events = [
        ({"type": "system", "subtype": "output_limit", "from": 32000, "to": 64000}, "output limit raised: 32,000 → 64,000 tokens"),
        ({"type": "system", "subtype": "act_reminder", "kind": "reasoning_cutoff", "reasoning_chars": 8000},
         "reasoning cutoff: told to act; 8,000 characters of its reasoning handed back"),
        ({"type": "system", "subtype": "api_retry", "attempt": 2, "retry_delay_ms": 12000, "error": "tls handshake eof"},
         "model call failed, retry 2 in 12s · tls handshake eof"),
        ({"type": "system", "subtype": "nudge", "kind": "thinking_time_box"}, "nudge: thinking time box"),
        ({"type": "system", "subtype": "compact_failed", "trigger": "auto", "reason": "empty_summary"},
         "compaction failed: empty_summary (auto)"),
        ({"type": "system", "subtype": "context_window_learned", "model": "m", "window": 100000, "prompt": 120000},
         "the server's window is 100,000 tokens (prompt 120,000)"),
    ]
    for event, words in events:
        assert words in text(chat.render(event)), event["subtype"]


def test_a_runs_error_is_shown_once():
    chat = watch.Chat()
    why = "Reached the time limit (--max-time): the run stopped; the work done so far is left as it is."
    end = {"type": "result", "subtype": "error_during_execution", "is_error": True, "num_turns": 8,
           "duration_ms": 869317, "errors": [why], "usage": {"output_tokens": 71658}}
    shown = text(chat.render(end))
    assert "✘ error_during_execution · 8 steps · 14m29s · 71,658 tokens out" in shown and why in shown
    assert watch.handle(why, True, None, None, chat.ended) == [], "not again from KodAct's stderr"
    assert watch.handle("API Error: network error: x. Retrying in 6s (attempt 1).", True) == [], "shown from its event"
    assert "unknown model" in text(watch.handle("error: unknown model", True))


def make_job(tmp: Path, events: list[dict], *, finished: bool) -> Path:
    job = tmp / "job"
    agent = job / "demo-task__AbC123" / "agent"
    agent.mkdir(parents=True)
    (job / "config.json").write_text(json.dumps({"n_concurrent_trials": 1, "agents": [{"model_name": "deep-thinking"}]}))
    (agent / "instruction.txt").write_text("Fix the bug in /app/x.py so the tests pass.")
    (agent / "run.json").write_text(json.dumps({"model": "deep-thinking", "flags": ["--max-time", "870", "--effort", "high"]}))
    (agent / "kodact.jsonl").write_text("".join(json.dumps(e) + "\n" for e in events))
    if finished:
        (job / "demo-task__AbC123" / "result.json").write_text("{}")
        (job / "demo-task__AbC123" / "verifier").mkdir()
        (job / "demo-task__AbC123" / "verifier" / "reward.txt").write_text("1")
    return job


def run_watch(args: list[str], *, heartbeat: int | None = None, seconds: float | None = None) -> str:
    """watch.py's output: with --once, until it stops; else for `seconds`, then stopped."""
    setup = f"watch.HEARTBEAT = {heartbeat}; " if heartbeat else ""
    code = (f"import sys; sys.path.insert(0, {str(HERE)!r}); import watch; {setup}"
            f"sys.argv = ['watch.py'] + {args!r}; watch.main()")
    env = dict(os.environ, NO_COLOR="1", COLUMNS="100", PATH=os.environ.get("PATH", ""))
    if seconds is None:
        return subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, env=env, timeout=60).stdout
    proc = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    time.sleep(seconds)
    proc.terminate()
    return proc.communicate(timeout=30)[0]


def test_a_job_is_shown_from_its_start_without_a_flag(tmp_path):
    events = [
        {"type": "system", "subtype": "init", "cwd": "/app", "tools": ["Bash"], "model": "deep-thinking"},
        assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "pytest -q"}}),
        result("t1", "1 passed", {"exitCode": 0}),
        {"type": "result", "subtype": "success", "is_error": False, "num_turns": 2, "duration_ms": 9000},
    ]
    job = make_job(tmp_path, events, finished=True)
    out = run_watch(["--once", str(job)])
    for words in ("┏━ demo-task AbC123", "model deep-thinking · time limit 14m30s · flags --effort high",
                  "Fix the bug in /app/x.py", "▸ step 1", "→ $ pytest -q", "✓ exit 0 · 1 line", "✔ finished · 2 steps",
                  "reward 1 ✔ passed · 1/1 passed so far"):
        assert words in out, words
    assert out.index("Fix the bug") < out.index("▸ step 1") < out.index("✔ finished") < out.index("reward 1"), "in order"
    assert "▸ step 1" not in run_watch(["--once", "--new", str(job)]), "--new: only what happens from now on"
    assert "demo-task" not in run_watch(["--once", "--task", "other*", str(job)])


def test_a_trial_at_work_says_what_it_is_waiting_for(tmp_path):
    events = [assistant({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "make -j8 all"}})]
    job = make_job(tmp_path, events, finished=False)
    out = run_watch([str(job)], heartbeat=1, seconds=4)
    assert "⋯ running $ make -j8 all ·" in out, out
    events.append(result("t1", "built", {"exitCode": 0}))
    job = make_job(tmp_path / "2", events, finished=False)
    out = run_watch([str(job)], heartbeat=1, seconds=4)
    assert "⋯ the model is working on step 2 ·" in out, out
