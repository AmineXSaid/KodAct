"""Tests for the reply limit a run gives its model: max-output.sh settles it, run.sh names the job
after it, and kodact_agent.py passes it to kodact as modelLimits.

    python3 -m pytest evals/harbor/test_max_output.py

What they hold: deep-thinking gets 65536 unless the person says otherwise; another model gets
none unless one is set; 0 or empty is none; anything that isn't a count stops the run; the job
name says the limit; and the agent's flags give it to kodact for the run's model only, unless the
person's own --settings are there.
"""

import ast
import json
import os
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent


def settle(**settings: str) -> tuple[str, str, int]:
    """KODACT_HARBOR_MAX_OUTPUT after max-output.sh, as "[value]" or "[unset]", its stderr and exit code."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    env.update(settings)
    script = f'set -euo pipefail; source "{HERE / "max-output.sh"}"; printf "[%s]" "${{KODACT_HARBOR_MAX_OUTPUT-unset}}"'
    p = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
    return p.stdout.splitlines()[-1] if p.stdout else "", p.stderr, p.returncode


def flags(model: str, extra: list[str], **settings: str) -> list[str]:
    """kodact_agent.model_limits_flags with these settings, read from the source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    fn = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "model_limits_flags")
    scope: dict = {"os": os, "json": json}
    exec(compile(ast.Module(body=[fn], type_ignores=[]), "kodact_agent.py", "exec"), scope)
    saved = {k: os.environ.pop(k) for k in list(os.environ) if k.startswith("KODACT_HARBOR_")}
    try:
        os.environ.update(settings)
        return scope["model_limits_flags"](model, extra)
    finally:
        for k in settings:
            os.environ.pop(k, None)
        os.environ.update(saved)


def test_deep_thinking_gets_65536_unless_told_otherwise():
    out, err, code = settle()
    assert (out, code) == ("[65536]", 0)
    assert settle(KODACT_HARBOR_MODEL="openai/deep-thinking")[0] == "[65536]", "a provider prefix is dropped"
    assert settle(KODACT_HARBOR_MAX_OUTPUT="131072")[0] == "[131072]"
    assert settle(KODACT_HARBOR_MAX_OUTPUT="0")[0] == "[unset]"
    assert settle(KODACT_HARBOR_MAX_OUTPUT="")[0] == "[unset]"


def test_another_model_gets_none_unless_one_is_set():
    assert settle(KODACT_HARBOR_MODEL="qwen-coder")[0] == "[unset]"
    assert settle(KODACT_HARBOR_MODEL="qwen-coder", KODACT_HARBOR_MAX_OUTPUT="16384")[0] == "[16384]"


def test_a_value_that_isnt_a_count_stops_the_run():
    out, err, code = settle(KODACT_HARBOR_MAX_OUTPUT="64k")
    assert code == 2 and "must be a number of tokens" in err and out == ""


def test_the_job_name_says_the_limit():
    run = (HERE / "run.sh").read_text()
    assert run.index("max-output.sh") < run.index('job="kodact-'), "settled before the job is named"
    lines = [ln for ln in run.splitlines() if ln.startswith('[ -n "${KODACT_HARBOR_')]
    script = 'ca=""\n' + "\n".join(lines) + '\nprintf "%s" "$ca"'
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    named = subprocess.run(["bash", "-c", script], env={**env, "KODACT_HARBOR_MAX_OUTPUT": "65536"}, capture_output=True, text=True)
    assert "-maxout65536" in named.stdout
    plain = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
    assert "-maxout" not in plain.stdout


def test_the_job_name_says_a_time_limit_set_by_hand():
    run = (HERE / "run.sh").read_text()
    lines = [ln for ln in run.splitlines() if ln.startswith('[ -n "${KODACT_HARBOR_')]
    script = 'ca=""\n' + "\n".join(lines) + '\nprintf "%s" "$ca"'
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    named = subprocess.run(["bash", "-c", script], env={**env, "KODACT_HARBOR_TIME_LIMIT": "2700"}, capture_output=True, text=True)
    assert named.stdout.endswith("-time2700"), named.stdout
    plain = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
    assert "-time" not in plain.stdout


def test_the_job_name_says_reasoning_is_carried():
    run = (HERE / "run.sh").read_text()
    lines = [ln for ln in run.splitlines() if ln.startswith('case "${KODACT_CARRY_REASONING')]
    assert len(lines) == 1, lines
    script = 'ca=""\n' + lines[0] + '\nprintf "%s" "$ca"'
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_")}
    # The values kodact itself reads as off (kodact-core's carry_reasoning) name no carry.
    for value, name in [("1", "-carry"), ("true", "-carry"), ("", ""), ("0", ""), ("false", ""), ("no", ""), ("off", "")]:
        r = subprocess.run(["bash", "-c", script], env={**env, "KODACT_CARRY_REASONING": value}, capture_output=True, text=True)
        assert r.stdout == name, (value, r.stdout)
    r = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True)
    assert r.stdout == "", r.stdout


def test_the_agent_gives_kodact_the_limit_for_the_runs_model():
    got = flags("deep-thinking", [], KODACT_HARBOR_MAX_OUTPUT="65536")
    assert got[0] == "--settings"
    assert json.loads(got[1]) == {"modelLimits": {"deep-thinking": {"maxOutputTokens": 65536}}}
    assert flags("deep-thinking", []) == [], "no limit, no settings"
    assert flags("", [], KODACT_HARBOR_MAX_OUTPUT="65536") == [], "no model to name"
    own = ["--settings", '{"modelLimits": {}}']
    assert flags("deep-thinking", own, KODACT_HARBOR_MAX_OUTPUT="65536") == [], "the person's own settings win"
