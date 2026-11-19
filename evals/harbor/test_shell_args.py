"""Tests for shell_args.split, the splitter of KODACT_HARBOR_ARGS and PI_HARBOR_ARGS.

    python3 -m pytest evals/harbor/test_shell_args.py

What they hold: every quoting bash knows ('…', "…", $'…', backslashes) gives the words bash
gives, text written by printf '%q' or shlex.quote comes back as it was, an unclosed quote is
refused, and so is a $(…) or backquote left unrun; run.sh stops before any trial on either.
"""

import os
import shlex
import subprocess
from pathlib import Path

import pytest

from shell_args import split

HERE = Path(__file__).resolve().parent
# Text like a skill: newlines, apostrophes, double quotes, backquotes, $, a tab, unicode.
SKILL = """---
name: i-have-adhd
description: 'Shape output: lead with the next action. Invoke with /i-have-adhd; stays on until "stop adhd mode".'
---

Bad: "Let's think about this..."
Good: "Edit `src/auth.ts:42`." Fix: add `Authorization: Bearer ${token}`\tand $(not run) — done.
"""


def bash_words(s: str) -> list[str]:
    """The words bash makes of s, quoting only (no expansion is used in these cases)."""
    # Bytes, not text=True: that would read a \r as \n.
    out = subprocess.run(["bash", "-c", f"printf '%s\\0' {s}"], capture_output=True, check=True).stdout
    return out.decode().split("\0")[:-1]


def percent_q(text: str) -> str:
    return subprocess.run(["bash", "-c", 'printf "%q" "$1"', "_", text], capture_output=True, text=True, check=True).stdout


@pytest.mark.parametrize("s", [
    "--max-turns 100",
    "  --a   'b c'  \"d e\" f\\ g ",
    "--x=\"a \\\"q\\\" \\\\ \\$ b\"",
    "a'b'\"c\"$'d\\ne'",
    "$'\\a\\b\\e\\f\\n\\r\\t\\v\\\\\\'\\\"\\?'",
    "$'\\101\\x42\\u00e9\\U0001F600\\cA\\q'",
    "'it'\\''s'",
    "a\\\nb",
    "''",
])
def test_same_words_as_bash(s):
    assert split(s) == bash_words(s)


@pytest.mark.parametrize("quote", [percent_q, shlex.quote])
def test_quoted_text_comes_back(quote):
    for text in (SKILL, "plain", "it's", "a\nb", "-- dash first", ""):
        assert split(f"--append-system-prompt {quote(text)} --verbose") == ["--append-system-prompt", text, "--verbose"]


def test_percent_q_of_the_skill_is_ansi_c():
    """The case that crashed: printf '%q' writes $'…' for text with a newline, which shlex can't read."""
    assert percent_q(SKILL).startswith("$'")
    with pytest.raises(ValueError):
        shlex.split(percent_q(SKILL))


@pytest.mark.parametrize("s", ["'open", '"open', "$'open", "$'open\\'"])
def test_unclosed_quote_is_refused(s):
    with pytest.raises(ValueError, match="No closing quotation"):
        split(s)


@pytest.mark.parametrize("s", ["--append-system-prompt $(cat SKILL.md)", '"$(cat x)"', "`cat x`"])
def test_command_substitution_left_unrun_is_refused(s):
    with pytest.raises(ValueError, match="command substitution"):
        split(s)


@pytest.mark.parametrize("s", ["'$(cat x)'", "$'$(cat x)'", "\\$HOME", '"\\$(x)"'])
def test_quoted_dollar_is_text(s):
    assert split(s) == bash_words(s)


def run_sh_check(args: str) -> subprocess.CompletedProcess:
    """run.sh's KODACT_HARBOR_ARGS check, the lines that run before any trial, on their own."""
    lines = (HERE / "run.sh").read_text().splitlines()
    start = next(i for i, l in enumerate(lines) if "shell_args.py" in l and l.startswith("if "))
    end = next(i for i in range(start, len(lines)) if lines[i] == "fi")
    script = f'repo="{HERE.parents[1]}"\n' + "\n".join(lines[start:end + 1]) + "\necho ok"
    return subprocess.run(["bash", "-c", script], env={**os.environ, "KODACT_HARBOR_ARGS": args}, capture_output=True, text=True)


def test_run_sh_accepts_percent_q_and_stops_on_bad_args():
    assert run_sh_check(f"--append-system-prompt {percent_q(SKILL)}").stdout.strip() == "ok"
    for bad in ("--append-system-prompt it's", "--append-system-prompt $(cat SKILL.md)"):
        p = run_sh_check(bad)
        assert p.returncode == 2 and "can't be split" in p.stderr
