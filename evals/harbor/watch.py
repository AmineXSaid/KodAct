#!/usr/bin/env python3
"""Follow a Harbor job's agent conversations live, as readable text.

    python3 evals/harbor/watch.py                 # the newest job under jobs/, and each newer one
                                                  # as it starts (a loop of jobs, one after another)
    python3 evals/harbor/watch.py jobs/<job-name> # a given job
    python3 evals/harbor/watch.py --from-start    # replay what's there, then follow
    python3 evals/harbor/watch.py --thinking      # also show the model's reasoning
    python3 evals/harbor/watch.py --full          # everything: reasoning, tool input and output

KodAct's stream-json (kodact.jsonl) is shown like a conversation: what the
model says, one short line per tool call, failed tool calls, and how the run
ended, and so is Pi's event stream (pi.jsonl, read through pi_stream.py). Any
other *.jsonl / *.stdout.txt / *.stderr.txt in a task's agent/ folder (Ante,
Forge Code, ...) is shown line by line as plain text. A task's
reward is shown when its verifier writes it; a trial that starts, and one that
ends in a Harbor error (a container that doesn't start, tests that time out),
are shown too, with whether the agent ran.

A step appears when the model finishes it (a reasoning model can be quiet for
minutes). Ctrl-C stops watching; the run itself isn't affected.
"""

import json
import re
import sys
import time
from pathlib import Path

from pi_stream import PiStream

COLORS = ["\033[36m", "\033[35m", "\033[33m", "\033[32m", "\033[34m", "\033[31m"]
DIM, BOLD, RED, GREEN, RESET = "\033[2m", "\033[1m", "\033[91m", "\033[92m", "\033[0m"
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
TAGS = re.compile(r"</?(system-reminder|tool_use_error)>")
PATTERNS = ["*.jsonl", "*.stdout.txt", "*.stderr.txt"]
KNOWN = {"assistant", "user", "result", "system"}
TEXT_LINES = 40  # lines kept from one message of the model
QUIET_SYSTEM = {None, "init", "compact_boundary"}

THINKING = "--thinking" in sys.argv or "--full" in sys.argv
FULL = "--full" in sys.argv
# Pi's tool names, as the ones tool_line() knows.
PI_TOOLS = {"bash": "Bash", "read": "Read", "write": "Write", "edit": "Edit", "grep": "Grep", "find": "Glob", "ls": "LS"}


def short(text, width: int = 160) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= width else text[: width - 1] + "…"


def first_line(text, width: int = 160) -> str:
    lines = [line for line in str(text).splitlines() if line.strip()]
    if not lines:
        return ""
    more = f" (+{len(lines) - 1} lines)" if len(lines) > 1 else ""
    return short(lines[0], width) + more


def tool_line(name: str, args: dict) -> str:
    """One readable line for a tool call: `$ ls -la`, `Read /app/x.py`, ..."""
    if not isinstance(args, dict):
        return name
    if "_truncated_input" in args:
        return f"{name} (input cut off)"
    path = args.get("file_path") or args.get("notebook_path") or args.get("path") or ""
    name = PI_TOOLS.get(name, name)
    if name == "Bash":
        command, why = str(args.get("command", "")), args.get("description")
        if "\n" in command.strip() and why:
            return f"$ {first_line(command, 60)} · {short(why, 80)}"
        return f"$ {first_line(command, 140)}"
    if name == "Write":
        n = len(str(args.get("content", "")).splitlines())
        return f"Write {path} ({n} lines)"
    if name in ("Read", "Edit", "MultiEdit", "NotebookEdit", "LS"):
        at = f":{args['offset']}" if name == "Read" and args.get("offset") else ""
        return f"{name} {path}{at}"
    if name in ("Glob", "Grep"):
        where = f" in {path}" if path else ""
        return f"{name} {short(args.get('pattern', ''), 80)}{where}"
    if name == "TodoWrite":
        todos = args.get("todos") or []
        done = sum(1 for t in todos if isinstance(t, dict) and t.get("status") == "completed")
        return f"Todos {done}/{len(todos)} done"
    if name in ("BashOutput", "KillShell"):
        return f"{name} {args.get('bash_id') or args.get('shell_id') or ''}".strip()
    if name in ("Task", "Agent"):
        return f"{name}: {short(args.get('description') or args.get('prompt', ''), 100)}"
    for value in args.values():
        if isinstance(value, str) and value.strip():
            return f"{name} {short(value, 100)}"
    return name


def tool_text(block: dict) -> str:
    body = block.get("content")
    if isinstance(body, list):
        body = " ".join(b.get("text", "") for b in body if isinstance(b, dict))
    return str(body or "")


def error_line(text: str) -> str:
    """A failed tool call in one line: for a command, its exit code and last line of output."""
    lines = [line.strip() for line in TAGS.sub("", text).splitlines() if line.strip()]
    if not lines:
        return "error"
    if re.fullmatch(r"Exit code \d+", lines[0]) and len(lines) > 1:
        return f"{lines[0]} · {short(lines[-1], 180)}"
    return short(lines[0], 200)


def finish_line(event: dict) -> str:
    turns, secs = event.get("num_turns"), (event.get("duration_ms") or 0) / 1000
    stats = f"{turns} turns · {secs:.0f}s"
    if event.get("subtype") == "success" and not event.get("is_error"):
        return f"{GREEN}{BOLD}✔ finished{RESET} · {stats}"
    errors = event.get("errors") or []
    why = errors[0] if errors else event.get("result") or event.get("stop_reason") or ""
    return f"{RED}{BOLD}✘ {event.get('subtype')}{RESET} · {stats} · {first_line(why, 200)}"


def render(event: dict, who: str = "KodAct") -> list[str]:
    kind = event.get("type")
    out = []
    if kind == "assistant":
        blocks = event.get("message", {}).get("content", [])
        if FULL:
            out.append(f"{BOLD}❖ {who}{RESET}")
        for block in blocks:
            t = block.get("type")
            if t == "thinking" and block.get("thinking") and THINKING:
                out += [f"{DIM}  ~ {line}{RESET}" for line in block["thinking"].strip().splitlines()[:TEXT_LINES]]
            elif t == "text" and block.get("text", "").strip():
                if not FULL:
                    out.append(f"{BOLD}❖ {who}{RESET}")
                lines = block["text"].strip().splitlines()
                out += lines[:TEXT_LINES]
                if len(lines) > TEXT_LINES:
                    out.append(f"{DIM}… {len(lines) - TEXT_LINES} more lines{RESET}")
            elif t == "tool_use":
                args = block.get("input", {})
                detail = f" {DIM}{short(json.dumps(args), 300)}{RESET}" if FULL else ""
                out.append(f"  {DIM}→{RESET} {tool_line(block.get('name', '?'), args)}{detail}")
    elif kind == "user":
        content = event.get("message", {}).get("content", [])
        for block in content if isinstance(content, list) else []:
            if block.get("type") == "tool_result":
                text = tool_text(block)
                moved = re.search(r"still running after (.+?)\. .*? in the background as (bash_\d+)", text)
                if block.get("is_error"):
                    out.append(f"  {RED}✗ {error_line(text)}{RESET}")
                elif moved:
                    out.append(f"  {DIM}↷ still running after {moved.group(1)}: moved to the background as {moved.group(2)}{RESET}")
                elif FULL:
                    out.append(f"  {DIM}← {short(text, 300)}{RESET}")
            elif block.get("type") == "text" and event.get("isSynthetic"):
                text = TAGS.sub("", block.get("text", "")).strip()
                out.append(f"  {DIM}· {short(text, 300 if FULL else 140)}{RESET}")
    elif kind == "result":
        out.append(finish_line(event))
    elif kind == "system" and event.get("subtype") == "api_retry":
        wait = (event.get("retry_delay_ms") or 0) / 1000
        why = short(event.get("error", ""), 140)
        out.append(f"  {RED}↻ model call failed, retry {event.get('attempt')} in {wait:.0f}s · {why}{RESET}")
    elif kind == "system" and event.get("subtype") == "background_wait":
        limit = (event.get("limit_ms") or 0) / 1000
        out.append(f"  {DIM}⏳ waiting for {', '.join(event.get('shells') or [])} (up to {limit / 60:.0f} min){RESET}")
    elif kind == "system" and event.get("subtype") == "background_exit":
        for sh in event.get("shells") or []:
            code = sh.get("exit_code")
            how = f"exit {code}" if code is not None else "ended by a signal"
            color = DIM if code == 0 else RED
            out.append(f"  {color}· {sh.get('id')} finished, {how} after {sh.get('seconds')}s: {short(sh.get('command', ''), 100)}{RESET}")
    elif kind == "system" and event.get("subtype") not in QUIET_SYSTEM:
        if FULL or not str(event.get("subtype", "")).endswith("_reminder"):
            out.append(f"  {DIM}· {event.get('subtype')}{RESET}")
    return out


def handle(line: str, is_err: bool, pi: PiStream | None = None) -> list[str]:
    """One log line -> display lines: stream-json events are rendered, anything else shown as text.

    `pi` translates the lines of a Pi event stream first.
    """
    line = ANSI.sub("", line).rstrip()
    if not line.strip():
        return []
    try:
        event = json.loads(line)
    except ValueError:
        event = None
    if pi is not None and isinstance(event, dict):
        return [text for ev in pi.feed(event) for text in render(ev, "Pi")]
    if isinstance(event, dict) and event.get("type") in KNOWN:
        return render(event)
    if is_err:
        return [f"{RED}err: {short(line, 400)}{RESET}"]
    return [line]


def reward_line(path: Path) -> str:
    try:
        value = float(path.read_text().strip())
    except (OSError, ValueError):
        return ""
    if value >= 1:
        return f"{GREEN}{BOLD}reward {value:g} ✔ passed{RESET}"
    return f"{RED}{BOLD}reward {value:g} ✘ failed{RESET}"


def newest_job() -> Path:
    jobs = [p for p in Path("jobs").glob("*") if p.is_dir()]
    if not jobs:
        sys.exit("no jobs/ directory with runs here; cd to the KodAct checkout or pass a job path")
    return max(jobs, key=lambda p: p.stat().st_mtime)


def trial_error(trial: Path) -> str:
    """How a trial ended in a Harbor error, from its exception.txt: the error's last line."""
    try:
        lines = [ln.strip() for ln in (trial / "exception.txt").read_text(errors="replace").splitlines() if ln.strip()]
    except OSError:
        return ""
    # The last line that names an error (`harbor.trial.errors.XError: ...`); text after it
    # (the agent's captured stdout or stderr) is not the error.
    errors = [ln for ln in lines if re.match(r"^[\w.]*(Error|Exception)\b(:|$)", ln)]
    if not errors:
        return ""
    last = errors[-1]
    name = last.split(":", 1)[0].rsplit(".", 1)[-1]
    ran = any((trial / "agent").glob("*.jsonl"))
    where = {
        "EnvironmentStartTimeoutError": "the container didn't start; the agent never ran",
        "VerifierTimeoutError": "the tests ran out of time; nothing was scored",
        "NonZeroAgentExitCodeError": "the agent exited with an error",
    }.get(name, "the agent ran" if ran else "the agent never ran")
    detail = last.split(":", 1)[1].strip() if ":" in last else ""
    # "Command failed (exit 1): <the whole agent command>": the exit status is what tells.
    detail = re.sub(r"^(Command failed \(exit \d+\)):.*", r"\1", detail)
    return f"{RED}{BOLD}✘ Harbor: {name}{RESET} · {where}" + (f" · {short(detail, 100)}" if detail else "")


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    from_start = "--from-start" in sys.argv
    follow_newest = not args
    job = Path(args[0]) if args else newest_job()
    print(f"watching {job}  (Ctrl-C to stop)\n")
    offsets: dict[Path, int] = {}
    colors: dict[str, str] = {}
    pending: dict[Path, str] = {}
    pi_streams: dict[Path, PiStream] = {}
    rewarded: set[Path] = set()
    started: set[Path] = set()
    failed: set[Path] = set()
    first_scan = True

    def show(task: str, text: str) -> None:
        color = colors.setdefault(task, COLORS[len(colors) % len(COLORS)])
        print(f"{color}[{task}]{RESET} {text}", flush=True)

    while True:
        # Without a job named: a newer job (the next of a loop) is followed from its start.
        if follow_newest and (newer := newest_job()) != job and newer.stat().st_mtime > job.stat().st_mtime:
            job = newer
            print(f"\n{BOLD}=== next job: {job}{RESET}\n", flush=True)
            for state in (offsets, pending, pi_streams):
                state.clear()
            rewarded.clear()
            started.clear()
            failed.clear()
            from_start = True
        for trial in sorted(p for p in job.glob("*__*") if p.is_dir()):
            task = trial.name.split("__")[0]
            if trial not in started:
                started.add(trial)
                if not first_scan or from_start:
                    show(task, f"{DIM}trial {trial.name} started{RESET}")
            if trial not in failed and (text := trial_error(trial)):
                failed.add(trial)
                show(task, text)
        found = {p for pat in PATTERNS for p in job.glob(f"*/agent/{pat}")}
        # A task's stderr after its other logs: an error usually ends what came before it.
        logs = sorted(found, key=lambda p: (p.parent.parent.name, p.name.endswith(".stderr.txt"), p.name))
        for log in logs:
            if log not in offsets:
                # Logs there at start are followed from their end; a task that starts later, from its beginning.
                offsets[log] = 0 if from_start or not first_scan else log.stat().st_size
            size = log.stat().st_size
            if size <= offsets[log]:
                continue
            with log.open(errors="replace") as f:
                f.seek(offsets[log])
                chunk = pending.pop(log, "") + f.read()
                offsets[log] = f.tell()
            lines = chunk.split("\n")
            if log.suffix == ".jsonl":
                if lines[-1]:
                    pending[log] = lines[-1]  # partial JSON line: finish it on the next read
                lines = lines[:-1]
            task = log.parent.parent.name.split("__")[0]
            is_err = log.name.endswith(".stderr.txt")
            pi = pi_streams.setdefault(log, PiStream()) if log.name == "pi.jsonl" else None
            for line in lines:
                for text in handle(line, is_err, pi):
                    show(task, text)
        first_scan = False
        for reward in sorted(job.glob("*/verifier/reward.txt")):
            if reward not in rewarded and (text := reward_line(reward)):
                rewarded.add(reward)
                show(reward.parent.parent.name.split("__")[0], text)
        time.sleep(1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
