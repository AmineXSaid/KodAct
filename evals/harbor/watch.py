#!/usr/bin/env python3
"""Follow a Harbor job's agent conversations live, as a readable chat.

    python3 evals/harbor/watch.py                    # the newest job under jobs/: what's there, then live;
                                                     # then each newer job as it starts (a loop of jobs)
    python3 evals/harbor/watch.py jobs/<job-name>    # a given job
    python3 evals/harbor/watch.py --task 'gpt2*'     # only the tasks matching (a glob; repeatable)
    python3 evals/harbor/watch.py --new              # only what happens from now on
    python3 evals/harbor/watch.py --once             # show what's there and stop
    python3 evals/harbor/watch.py --thinking         # the model's whole reasoning, not its start and end
    python3 evals/harbor/watch.py --full             # everything uncut: reasoning, commands, files, output
    python3 evals/harbor/watch.py --compact          # one short line per tool call, no output

Each trial is shown as a chat, under a header with its task, time limit and
instruction: every step of the model (numbered, with its output tokens and, when
seen live, how long it took), the start and end of its reasoning, what it says,
each tool call in full (the command, the start of a file it writes, the lines an
edit changes), each result (exit code and the first and last lines of the
output; red when it failed), KodAct's reminders and events (retries, output
limit, compaction, nudges), how the run ended and the task's reward. While the
model thinks or a command runs, a line every minute says so and for how long.

KodAct's stream-json (kodact.jsonl) and Pi's event stream (pi.jsonl, read
through pi_stream.py) are shown this way. Any other *.jsonl / *.stdout.txt /
*.stderr.txt in a task's agent/ folder (Ante, Forge Code, ...) is shown line by
line as plain text. A trial that ends in a Harbor error (a container that
doesn't start, tests that time out) is shown too, with whether the agent ran.

A step appears when the model finishes it. Ctrl-C stops watching; the run itself
isn't affected.
"""

import argparse
import fnmatch
import json
import os
import re
import shutil
import subprocess
import sys
import textwrap
import time
from pathlib import Path

from pi_stream import PiStream

COLOR = sys.stdout.isatty() and not os.environ.get("NO_COLOR")


def _c(code: str) -> str:
    return code if COLOR else ""


COLORS = [_c(c) for c in ("\033[36m", "\033[35m", "\033[33m", "\033[32m", "\033[34m", "\033[31m")]
DIM, BOLD, ITALIC, RESET = _c("\033[2m"), _c("\033[1m"), _c("\033[3m"), _c("\033[0m")
RED, GREEN, YELLOW, CYAN = _c("\033[91m"), _c("\033[92m"), _c("\033[93m"), _c("\033[96m")
ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
TAGS = re.compile(r"</?(system-reminder|tool_use_error|reasoning_start|reasoning_end)>")
PATTERNS = ["*.jsonl", "*.stdout.txt", "*.stderr.txt"]
KNOWN = {"assistant", "user", "result", "system"}
QUIET_SYSTEM = {None, "init", "compact_boundary"}  # --compact leaves these out
HIDDEN = {"type", "subtype", "session_id", "uuid"}
# Pi's tool names, as the ones the renderers know.
PI_TOOLS = {"bash": "Bash", "read": "Read", "write": "Write", "edit": "Edit", "grep": "Grep", "find": "Glob", "ls": "LS"}

# How much of each thing is shown by default; --full shows everything, --thinking all of the reasoning.
TEXT_LINES = 60  # of what the model says in one step
THINK_HEAD, THINK_TAIL = 4, 2  # of its reasoning: the start and the end
CMD_LINES = 15  # of a command
PREVIEW_LINES = 10  # of a file it writes, and of each side of an edit
OUT_HEAD, OUT_TAIL = 8, 4  # of a tool's output: the start and the end
NOTE_LINES = 6  # of a KodAct reminder
TASK_LINES = 8  # of the task's instruction
HEARTBEAT = 60  # seconds without news before (and between) "still thinking" lines
# kodact_agent.py's own flags, the same in every trial: left out of a trial's header.
FLAG_VALUES = {"--output-format", "--model", "--max-time"}
FLAG_SWITCHES = {"--dangerously-skip-permissions", "--verbose", "--autonomous"}


class Options:
    """What to show, from the command line (main)."""

    thinking = False
    full = False
    compact = False


OPT = Options()


def limit(n: int | None) -> int | None:
    """A number of lines to show, or None (all of them) with --full."""
    return None if OPT.full else n


def width() -> int:
    """Columns for the text of a line, after the trial's gutter."""
    return max(40, shutil.get_terminal_size((120, 40)).columns - 3)


def short(text, width: int = 160) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= width else text[: width - 1] + "…"


def first_line(text, width: int = 160) -> str:
    lines = [line for line in str(text).splitlines() if line.strip()]
    if not lines:
        return ""
    more = f" (+{len(lines) - 1} lines)" if len(lines) > 1 else ""
    return short(lines[0], width) + more


def clip(line, w: int) -> str:
    """One line of code or output, cut to `w` columns."""
    line = ANSI.sub("", str(line)).expandtabs(4).rstrip()
    return line if len(line) <= w else line[: max(1, w - 1)] + "…"


def wrap(text, w: int) -> list[str]:
    """Prose wrapped to `w` columns; a list item's next lines stay under its text."""
    out = []
    for para in ANSI.sub("", str(text)).expandtabs(4).splitlines():
        if not para.strip():
            out.append("")
            continue
        body = para.lstrip()
        item = re.match(r"([-*•]|\d+[.)])\s+", body)
        hang = " " * (len(para) - len(body) + (len(item.group(0)) if item else 0))
        out += textwrap.wrap(para, width=max(10, w), subsequent_indent=hang, break_on_hyphens=False) or [""]
    while out and not out[-1]:
        out.pop()
    while out and not out[0]:
        out.pop(0)
    return out


def cut(lines: list, head: int | None, tail: int = 0) -> list:
    """`lines` cut to their first `head` and last `tail`; the number left out between, as an int."""
    if head is None or len(lines) <= head + tail + 1:
        return list(lines)
    return lines[:head] + [len(lines) - head - tail] + (lines[len(lines) - tail :] if tail else [])


def more(n: int, what: str = "lines") -> str:
    return f"{DIM}… {n:,} more {what}{RESET}"


def dur(secs: float) -> str:
    secs = int(round(secs))
    if secs < 60:
        return f"{secs}s"
    m, s = divmod(secs, 60)
    if m < 60:
        return f"{m}m{s:02d}s"
    h, m = divmod(m, 60)
    return f"{h}h{m:02d}m"


def num(value) -> str:
    return f"{value:,}" if isinstance(value, int) else str(value)


def count(n: int, what: str = "line") -> str:
    return f"{n:,} {what}{'' if n == 1 else 's'}"


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


# ---- the chat view (the default) -------------------------------------------------------------


def output_lines(lines: list[str], color: str = DIM, head: int | None = OUT_HEAD, tail: int = OUT_TAIL) -> list[str]:
    """A tool's output under its call: the first and last lines, with how many are left out."""
    w = width() - 8
    out = []
    for item in cut(lines, limit(head), tail):
        if isinstance(item, int):
            out.append(f"    {DIM}┆ … {item:,} more lines{RESET}")
        else:
            out.append(f"    {DIM}│{RESET} {color}{clip(item, w)}{RESET}")
    return out


def numbered(lines: list[str], n: int | None) -> list[str]:
    """The start of a file being written, with line numbers."""
    w = width() - 12
    shown = lines if n is None else lines[:n]
    out = [f"    {DIM}{i:>4} │{RESET} {clip(line, w)}" for i, line in enumerate(shown, 1)]
    if len(lines) > len(shown):
        out.append(f"         {more(len(lines) - len(shown))}")
    return out


def diff(old, new, n: int | None) -> list[str]:
    """The lines an edit takes out and puts in."""
    w = width() - 8
    out = []
    for sign, color, text in (("-", RED, old), ("+", GREEN, new)):
        lines = str(text or "").splitlines() or [""]
        shown = lines if n is None else lines[:n]
        out += [f"    {color}{sign} {clip(line, w)}{RESET}" for line in shown]
        if len(lines) > len(shown):
            out.append(f"      {more(len(lines) - len(shown))}")
    return out


def call_lines(name: str, args) -> list[str]:
    """A tool call in full: the whole command, the start of a file it writes, the lines an edit changes."""
    name = PI_TOOLS.get(name, name)
    arrow = f"{CYAN}→{RESET}"
    w = width() - 4
    if not isinstance(args, dict):
        return [f"{arrow} {BOLD}{name}{RESET}"]
    if "_truncated_input" in args:
        return [f"{arrow} {BOLD}{name}{RESET} {RED}(input cut off){RESET}"]
    path = args.get("file_path") or args.get("notebook_path") or args.get("path") or ""
    if name == "Bash":
        lines = str(args.get("command", "")).rstrip("\n").splitlines() or [""]
        notes = [str(args["description"]).strip()] if str(args.get("description") or "").strip() else []
        if args.get("run_in_background"):
            notes.append("in the background")
        note = f"  {DIM}# {' · '.join(notes)}{RESET}" if notes else ""
        plain_note = f"  # {' · '.join(notes)}" if notes else ""
        if len(lines) == 1 and len(lines[0]) + len(plain_note) + 2 <= w:
            return [f"{arrow} {CYAN}${RESET} {lines[0]}{note}"]
        out = [f"{arrow} {CYAN}${RESET} {clip(lines[0], w - 2)}"]
        for item in cut(lines[1:], limit(CMD_LINES)):
            out.append(f"    {more(item)}" if isinstance(item, int) else f"    {clip(item, w - 2)}")
        if notes:
            out.append(f"  {DIM}# {clip(' · '.join(notes), w - 4)}{RESET}")
        return out
    if name == "Write":
        body = str(args.get("content", "")).splitlines()
        return [f"{arrow} {BOLD}Write{RESET} {path} {DIM}· {count(len(body))}{RESET}"] + numbered(body, limit(PREVIEW_LINES))
    if name == "Edit":
        every = f" {DIM}(every match){RESET}" if args.get("replace_all") else ""
        return [f"{arrow} {BOLD}Edit{RESET} {path}{every}"] + diff(args.get("old_string"), args.get("new_string"), limit(PREVIEW_LINES))
    if name == "MultiEdit":
        edits = [e for e in args.get("edits") or [] if isinstance(e, dict)]
        out = [f"{arrow} {BOLD}MultiEdit{RESET} {path} {DIM}· {len(edits)} edits{RESET}"]
        shown = edits if OPT.full else edits[:3]
        for e in shown:
            out += diff(e.get("old_string"), e.get("new_string"), limit(4))
        if len(edits) > len(shown):
            out.append(f"    {more(len(edits) - len(shown), 'edits')}")
        return out
    if name == "TodoWrite":
        todos = [t for t in args.get("todos") or [] if isinstance(t, dict)]
        done = sum(t.get("status") == "completed" for t in todos)
        out = [f"{arrow} {BOLD}Todos{RESET} {DIM}{done}/{len(todos)} done{RESET}"]
        marks = {"completed": f"{GREEN}☑{RESET}", "in_progress": f"{YELLOW}▸{RESET}"}
        for t in todos:
            out.append(f"    {marks.get(t.get('status'), '☐')} {clip(t.get('content', ''), w - 6)}")
        return out
    if name in ("Task", "Agent"):
        out = [f"{arrow} {BOLD}{name}{RESET} {short(args.get('description', ''), w - 10)}"]
        for item in cut(wrap(args.get("prompt", ""), w - 4), limit(3)):
            out.append(f"    {more(item)}" if isinstance(item, int) else f"    {DIM}{item}{RESET}")
        return out
    return [f"{arrow} {tool_line(name, args)}"]


def result_lines(name: str, block: dict, meta) -> list[str]:
    """A tool's result under its call: whether it worked, and what it printed."""
    name = PI_TOOLS.get(name, name)
    meta = meta if isinstance(meta, dict) else {}
    text = TAGS.sub("", tool_text(block)).rstrip()
    lines = text.splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    w = width() - 6
    if block.get("is_error"):
        head, rest = (lines[0].strip(), lines[1:]) if lines else ("error", [])
        if re.fullmatch(r"Exit code \d+", head):
            head = head.replace("Exit code", "exit")
        return [f"  {RED}✗ {clip(head, w)}{RESET}"] + output_lines(rest, RED)
    moved = re.search(r"still running after (.+?)\. .*? in the background as (bash_\d+)", text, re.S)
    if moved or meta.get("backgroundTaskId"):
        after = f"after {moved.group(1)}" if moved else "past its wait"
        shell = moved.group(2) if moved else meta.get("backgroundTaskId")
        return [f"  {YELLOW}↷{RESET} {DIM}still running {after}: goes on in the background as {shell}{RESET}"]
    ok = f"{GREEN}✓{RESET}"
    if name in ("Bash", "BashOutput"):
        if meta.get("interrupted"):
            return [f"  {RED}✗ interrupted{RESET}"] + output_lines(lines, RED)
        status = f"exit {meta['exitCode']}" if "exitCode" in meta else "done"
        size = count(len(lines)) if lines else "no output"
        return [f"  {ok} {DIM}{status} · {size}{RESET}"] + output_lines(lines)
    if name == "Read":
        info = meta.get("file") if isinstance(meta.get("file"), dict) else {}
        n, total, start = info.get("numLines") or len(lines), info.get("totalLines"), info.get("startLine") or 1
        span = f"lines {start}–{start + n - 1} of {total:,}" if total and n < total else count(n)
        return [f"  {ok} {DIM}read {span}{RESET}"] + (output_lines(lines) if OPT.full else [])
    if name in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        word = {"create": "created", "update": "updated"}.get(meta.get("type"), "written" if name == "Write" else "edited")
        return [f"  {ok} {DIM}{word}{RESET}"]
    if name == "TodoWrite":
        return []
    size = count(len(lines)) if lines else "no output"
    return [f"  {ok} {DIM}{size}{RESET}"] + output_lines(lines, head=6, tail=2)


def thinking_lines(text: str) -> list[str]:
    """The model's reasoning: its start and its end, or all of it with --thinking."""
    lines = [line for line in wrap(text.strip(), width() - 4) if line.strip()]
    whole = OPT.thinking or OPT.full
    out = []
    for i, item in enumerate(cut(lines, None if whole else THINK_HEAD, 0 if whole else THINK_TAIL)):
        if isinstance(item, int):
            out.append(f"  {DIM}┆ … {item:,} more lines of reasoning ({len(text):,} characters in all){RESET}")
        else:
            out.append(f"{DIM}{ITALIC}{'∴ ' if i == 0 else '  '}{item}{RESET}")
    return out


def text_lines(who: str, text: str) -> list[str]:
    out = [f"{BOLD}❖ {who}{RESET}"]
    for item in cut(wrap(text.strip(), width() - 4), limit(TEXT_LINES)):
        out.append(f"  {more(item)}" if isinstance(item, int) else f"  {item}")
    return out


def note_lines(text: str) -> list[str]:
    """Something KodAct told the model (a reminder, the time left): its first lines."""
    text = TAGS.sub("", text).strip()
    if text.startswith("[") and text.endswith("]") and "time limit" in text:
        return [f"{RED}{BOLD}⏱ {text.strip('[]')}{RESET}"]
    out = []
    lines = [line for line in wrap(text, width() - 4) if line.strip()]
    for i, item in enumerate(cut(lines, limit(NOTE_LINES))):
        if isinstance(item, int):
            out.append(f"  {more(item)}")
        else:
            out.append(f"{YELLOW}⚑{RESET} {DIM}{item}{RESET}" if i == 0 else f"  {DIM}{item}{RESET}")
    return out


def system_lines(ev: dict) -> list[str]:
    """One of KodAct's own events: a retry, a raised output limit, a compaction, a nudge, ..."""
    sub, get = ev.get("subtype"), ev.get
    if sub == "init":
        tools = get("tools") or []
        return [f"{DIM}session started · model {get('model')} · {len(tools)} tools · in {get('cwd')}{RESET}"]
    if sub == "api_retry":
        wait = (get("retry_delay_ms") or 0) / 1000
        return [f"{RED}↻ model call failed, retry {get('attempt')} in {wait:.0f}s · {short(get('error', ''), 140)}{RESET}"]
    if sub == "output_limit":
        return [f"{YELLOW}⚙{RESET} {DIM}output limit raised: {num(get('from'))} → {num(get('to'))} tokens{RESET}"]
    if sub == "act_reminder":
        kind = str(get("kind") or "").replace("_", " ")
        chars = get("reasoning_chars")
        kept = f"; {num(chars)} characters of its reasoning handed back" if chars else ""
        return [f"{YELLOW}⚑{RESET} {DIM}{kind}: told to act{kept}{RESET}"]
    if sub == "nudge":
        rest = ", ".join(f"{k} {short(v, 40)}" for k, v in ev.items() if k not in HIDDEN | {"kind"})
        return [f"{YELLOW}⚑{RESET} {DIM}nudge: {str(get('kind') or '').replace('_', ' ')}{(' · ' + rest) if rest else ''}{RESET}"]
    if sub == "context_window_learned":
        return [f"{YELLOW}⚙{RESET} {DIM}the server's window is {num(get('window'))} tokens (prompt {num(get('prompt'))}): KodAct uses it from now on{RESET}"]
    if sub == "reply_shrunk":
        return [f"{YELLOW}⚙{RESET} {DIM}reply limit lowered {num(get('from'))} → {num(get('to'))} tokens to fit the window ({num(get('window'))}){RESET}"]
    if sub == "model_fallback":
        return [f"{YELLOW}⚙{RESET} {DIM}model {get('from')} → {get('to')}: {short(get('reason', ''), 100)}{RESET}"]
    if sub == "compact_boundary":
        meta = get("compact_metadata") or get("compactMetadata") or {}
        return [f"{YELLOW}⚙{RESET} {DIM}conversation compacted ({meta.get('trigger', 'auto')}){RESET}"]
    if sub == "compact_failed":
        return [f"{RED}✗ compaction failed: {get('reason')} ({get('trigger')}){RESET}"]
    if sub == "background_wait":
        mins = (get("limit_ms") or 0) / 60000
        return [f"{DIM}⏳ waiting for {', '.join(get('shells') or [])} (up to {mins:.0f} min){RESET}"]
    if sub == "background_exit":
        out = []
        for sh in get("shells") or []:
            code = sh.get("exit_code")
            how = f"exit {code}" if code is not None else "ended by a signal"
            color = DIM if code == 0 else RED
            out.append(f"{color}· {sh.get('id')} finished, {how} after {sh.get('seconds')}s: {short(sh.get('command', ''), 100)}{RESET}")
        return out
    rest = ", ".join(f"{k} {short(v if isinstance(v, str) else json.dumps(v), 60)}" for k, v in ev.items() if k not in HIDDEN)
    return [f"{DIM}· {str(sub).replace('_', ' ')}{(': ' + rest) if rest else ''}{RESET}"]


def finish_lines(ev: dict) -> list[str]:
    """How the run ended: steps, time, output tokens, and the error when it failed."""
    secs = (ev.get("duration_ms") or 0) / 1000
    out_tokens = (ev.get("usage") or {}).get("output_tokens")
    stats = [f"{ev.get('num_turns')} steps" if ev.get("num_turns") is not None else "", dur(secs) if secs else ""]
    stats.append(f"{out_tokens:,} tokens out" if out_tokens else "")
    stats = " · ".join(s for s in stats if s)
    out = []
    if ev.get("subtype") == "success" and not ev.get("is_error"):
        out.append(f"{GREEN}{BOLD}✔ finished{RESET} {DIM}· {stats}{RESET}")
    else:
        errors = ev.get("errors") or []
        why = errors[0] if errors else ev.get("result") or ev.get("stop_reason") or ""
        out.append(f"{RED}{BOLD}✘ {ev.get('subtype')}{RESET} {DIM}· {stats}{RESET}")
        if why:
            out.append(f"  {RED}{first_line(why, width() - 4)}{RESET}")
    check = ev.get("verification")
    if isinstance(check, dict) and check.get("status"):
        out.append(f"  {DIM}{check_line(check)}{RESET}")
        if check.get("missing"):
            out.append(f"  {YELLOW}files the task asks for that don't exist: {', '.join(check['missing'])}{RESET}")
    return out


def check_line(check: dict) -> str:
    """KodAct's record of the changes at the end (`verification`), and what its last check ran:
    a pass is that command passing, not the task's result."""
    status, command = check.get("status"), check.get("command")
    files = ", ".join(check.get("files") or [])
    if status in ("passed", "failed") and command:
        return f"KodAct's last check {status}: $ {first_line(command, width() - 30)}"
    if status == "ran" and command:
        return f"KodAct's last check ran, its exit code hidden by the command: $ {first_line(command, width() - 50)}"
    if status == "unchecked":
        return "KodAct: changes no check has run on since: " + (files or "made by shell commands")
    return {"no_changes": "KodAct: no file changed", "read_back": "KodAct: data files read back after their last change"}.get(
        status, f"KodAct's check: {status}"
    )


class Chat:
    """One agent log shown as a conversation: steps numbered, each result under the call it answers."""

    def __init__(self, who: str = "KodAct"):
        self.who = who
        self.steps = 0
        self.calls: dict = {}  # tool_use_id -> (name, input)
        self.running: dict = {}  # the calls not answered yet
        self.multi = False  # the last step made more than one call: say which one a result answers
        self.done = False
        self.took: float | None = None  # seconds the next step took, when seen live
        self.took_over = False  # its start wasn't seen: it took at least `took`
        self.events = 0
        self.ended: set[str] = set()  # the errors the run ended with, as KodAct also prints them

    def render(self, ev: dict) -> list[str]:
        self.events += 1
        kind = ev.get("type")
        if kind == "assistant":
            return self.assistant(ev)
        if kind == "user":
            return self.user(ev)
        if kind == "result":
            self.done = True
            self.running.clear()
            self.ended |= {str(e).strip() for e in ev.get("errors") or [] if str(e).strip()}
            if ev.get("is_error") and str(ev.get("result") or "").strip():
                self.ended.add(str(ev["result"]).strip())
            return finish_lines(ev)
        if kind == "system":
            return system_lines(ev)
        return []

    def assistant(self, ev: dict) -> list[str]:
        msg = ev.get("message") or {}
        sub = bool(ev.get("parent_tool_use_id"))
        if not sub:
            self.steps += 1
        facts = []
        usage = msg.get("usage") or {}
        tokens = usage.get("output_tokens")
        if tokens:
            facts.append(f"{tokens:,} tokens out")
        # What the model read for this step: the conversation so far, cached or not.
        context = sum(usage.get(k) or 0 for k in ("input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"))
        if context:
            facts.append(f"context {context / 1000:,.0f}k" if context >= 1000 else f"context {context}")
        if self.took is not None and self.took >= 1:
            facts.append(("over " if self.took_over else "") + dur(self.took))
        self.took, self.took_over = None, False
        label = "sub-agent step" if sub else f"step {self.steps}"
        out = [f"{BOLD}▸ {label}{RESET}" + (f" {DIM}· {' · '.join(facts)}{RESET}" if facts else "")]
        calls = []
        for block in msg.get("content") or []:
            kind = block.get("type")
            if kind == "thinking" and str(block.get("thinking") or "").strip():
                out += thinking_lines(str(block["thinking"]))
            elif kind == "text" and str(block.get("text") or "").strip():
                out += text_lines(self.who, str(block["text"]))
            elif kind == "tool_use":
                name, args = block.get("name", "?"), block.get("input") or {}
                self.calls[block.get("id")] = self.running[block.get("id")] = (name, args)
                calls.append(block)
                out += call_lines(name, args)
        if not sub:
            self.multi = len(calls) > 1
        if msg.get("stop_reason") == "max_tokens":
            out.append(f"{RED}✗ cut off at the output limit{' before any answer or action' if not calls else ''}{RESET}")
        return ["  " + line for line in out] if sub else out

    def user(self, ev: dict) -> list[str]:
        content = (ev.get("message") or {}).get("content")
        blocks = [{"type": "text", "text": content}] if isinstance(content, str) else content or []
        results = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_result"]
        meta = ev.get("tool_use_result") if len(results) == 1 else None
        out = []
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_result":
                name, args = self.calls.get(block.get("tool_use_id"), ("?", {}))
                self.running.pop(block.get("tool_use_id"), None)
                lines = result_lines(name, block, meta)
                if lines and self.multi:
                    lines[0] += f" {DIM}← {short(tool_line(name, args), 50)}{RESET}"
                out += lines
            elif block.get("type") == "text" and str(block.get("text") or "").strip():
                if ev.get("isSynthetic"):
                    out += note_lines(str(block["text"]))
                else:
                    out += [f"{BOLD}❖ user{RESET}"] + [f"  {line}" for line in wrap(block["text"], width() - 4)]
        return ["  " + line for line in out] if ev.get("parent_tool_use_id") else out

    def waiting(self, quiet: float, over: bool = False) -> str:
        """A line for a trial with no news for `quiet` seconds: what it is waiting for. `over`:
        the wait began before the watch could tell when, so it is at least that long."""
        so_far = ("over " if over else "") + f"{dur(quiet)} so far"
        if self.running:
            name, args = next(iter(self.running.values()))
            return f"{DIM}⋯ running {short(tool_line(name, args), 70)} · {so_far}{RESET}"
        return f"{DIM}⋯ the model is working on step {self.steps + 1} · {so_far}{RESET}"


# ---- the compact view (--compact): one short line per tool call ------------------------------


def render_compact(event: dict, who: str = "KodAct") -> list[str]:
    kind = event.get("type")
    out = []
    if kind == "assistant":
        for block in event.get("message", {}).get("content", []):
            t = block.get("type")
            if t == "thinking" and block.get("thinking") and OPT.thinking:
                out += [f"{DIM}  ~ {line}{RESET}" for line in block["thinking"].strip().splitlines()[:40]]
            elif t == "text" and block.get("text", "").strip():
                out.append(f"{BOLD}❖ {who}{RESET}")
                lines = block["text"].strip().splitlines()
                out += lines[:40]
                if len(lines) > 40:
                    out.append(more(len(lines) - 40))
            elif t == "tool_use":
                out.append(f"  {DIM}→{RESET} {tool_line(block.get('name', '?'), block.get('input', {}))}")
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
            elif block.get("type") == "text" and event.get("isSynthetic"):
                out.append(f"  {DIM}· {short(TAGS.sub('', block.get('text', '')).strip(), 140)}{RESET}")
    elif kind == "result":
        out += finish_lines(event)[:1]
    elif kind == "system" and event.get("subtype") not in QUIET_SYSTEM:
        if event.get("subtype") in ("api_retry", "background_wait", "background_exit"):
            out += system_lines(event)
        elif not str(event.get("subtype", "")).endswith("_reminder"):
            out.append(f"  {DIM}· {event.get('subtype')}{RESET}")
    return out


# ---- reading the logs ------------------------------------------------------------------------


def handle(line: str, is_err: bool, chat: "Chat | None" = None, pi: PiStream | None = None, shown=()) -> list[str]:
    """One log line -> display lines: stream-json events are rendered, anything else shown as text.

    `pi` translates the lines of a Pi event stream first; a stderr line in `shown` (the run's
    error, shown from its result event already) is left out.
    """
    line = ANSI.sub("", line).rstrip()
    if not line.strip():
        return []
    try:
        event = json.loads(line)
    except ValueError:
        event = None
    events = None
    if pi is not None and isinstance(event, dict):
        events = pi.feed(event)
    elif isinstance(event, dict) and event.get("type") in KNOWN:
        events = [event]
    if events is not None:
        chat = chat or Chat()
        return [text for ev in events for text in (render_compact(ev, chat.who) if OPT.compact else chat.render(ev))]
    if isinstance(event, dict):
        return [] if OPT.compact else [f"{DIM}· {event.get('type', 'event')}{RESET}"]
    if is_err:
        # KodAct's own notices; a retry is shown from its event in the stream already.
        if re.search(r"Retrying in \d+s \(attempt \d+\)", line) or line.strip() in shown:
            return []
        if re.search(r"\b(error|failed|refused|denied|panicked)\b", line, re.I):
            return [f"{RED}! {short(line, 400)}{RESET}"]
        return [f"{DIM}⚙ {short(line, 400)}{RESET}"]
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


# The agent's logs as KodAct writes them in the container (see kodact_agent.py).
LIVE_LOGS = ["kodact.jsonl", "kodact.stderr.txt"]
DOCKER = shutil.which("docker")
_containers: dict[Path, str | None] = {}


def container(trial: Path) -> str | None:
    """The running container of a trial: Harbor names its compose project `<trial>__env`
    (lower case) and the task's container is the service `main`."""
    if trial not in _containers or _containers[trial] is None:
        project = re.sub(r"[^a-z0-9_-]", "-", f"{trial.name}__env".lower())
        try:
            out = subprocess.run(
                [DOCKER, "ps", "-q", "--filter", f"label=com.docker.compose.project={project}",
                 "--filter", "label=com.docker.compose.service=main"],
                capture_output=True, text=True, timeout=10,
            ).stdout.split()
        except (OSError, subprocess.SubprocessError):
            out = []
        _containers[trial] = out[0] if out else None
    return _containers[trial]


def read_from(log: Path, offset: int) -> bytes | None:
    """What a log has past `offset`, or None when there is nothing new or it can't be read.
    While a trial runs, Harbor's bind-mounted logs belong to the container's user and this
    machine's can't read them, so they are read from inside the container."""
    try:
        if log.stat().st_size <= offset:
            return None
        with log.open("rb") as f:
            f.seek(offset)
            return f.read()
    except PermissionError:
        pass
    except FileNotFoundError:
        if not DOCKER:
            return None
    trial = log.parent.parent
    if not DOCKER or (trial / "result.json").exists() or not (cid := container(trial)):
        return None
    try:
        r = subprocess.run(
            [DOCKER, "exec", cid, "sh", "-c", f'tail -c +{offset + 1} "/logs/agent/{log.name}" 2>/dev/null'],
            capture_output=True, timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if r.returncode != 0:
        _containers[trial] = None  # gone, or not yet up: look it up again next time
    return r.stdout or None


def size_of(log: Path) -> int:
    """A log's size now, from here or from the trial's container."""
    try:
        return log.stat().st_size
    except OSError:
        data = read_from(log, 0)
        return len(data) if data else 0


def changed_at(log: Path) -> float | None:
    """When a log was last written to: from here, or, while its trial runs and its logs are the
    container user's, from inside the container (one clock: the container shares this machine's).
    None when neither can tell."""
    try:
        return log.stat().st_mtime
    except OSError:
        pass
    trial = log.parent.parent
    if not DOCKER or (trial / "result.json").exists() or not (cid := container(trial)):
        return None
    try:
        r = subprocess.run([DOCKER, "exec", cid, "stat", "-c", "%Y", f"/logs/agent/{log.name}"],
                           capture_output=True, text=True, timeout=10)
        return float(r.stdout.strip()) if r.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


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


def trial_info(trial: Path) -> list[str] | None:
    """A trial's header: the model, time limit, nudges, extra flags and the task's instruction,
    once the agent has written them (None until then)."""
    data = read_from(trial / "agent" / "instruction.txt", 0)
    if data is None:
        return None
    try:
        run = json.loads(read_from(trial / "agent" / "run.json", 0) or b"{}")
    except ValueError:
        run = {}
    flags = [str(f) for f in run.get("flags") or []]
    facts = [f"model {run['model']}"] if run.get("model") else []
    extra, i = [], 0
    while i < len(flags):
        flag = flags[i]
        if flag == "--max-time" and i + 1 < len(flags) and re.fullmatch(r"\d+(\.\d+)?", flags[i + 1]):
            facts.append(f"time limit {dur(float(flags[i + 1]))}")
        if flag in FLAG_VALUES:
            i += 2
            continue
        if flag not in FLAG_SWITCHES:
            extra.append(flag)
        i += 1
    settings = run.get("settings") or {}
    if settings.get("KODACT_NUDGES"):
        facts.append(f"nudges {settings['KODACT_NUDGES']}")
    if extra:
        facts.append(f"flags {' '.join(extra)}")
    out = [f"{DIM}{' · '.join(facts)}{RESET}"] if facts else []
    out.append(f"{BOLD}task{RESET}")
    for item in cut(wrap(data.decode(errors="replace"), width() - 4), limit(TASK_LINES)):
        out.append(f"  {more(item)}" if isinstance(item, int) else f"  {DIM}{item}{RESET}")
    return out


def job_line(job: Path) -> str:
    try:
        config = json.loads((job / "config.json").read_text())
    except (OSError, ValueError):
        return f"watching {job}"
    agents = config.get("agents") or [{}]
    model = agents[0].get("model_name") if isinstance(agents[0], dict) else None
    parts = [f"watching {BOLD}{job}{RESET}"]
    if model:
        parts.append(f"model {model}")
    if config.get("n_concurrent_trials"):
        n = config["n_concurrent_trials"]
        parts.append(f"{n} trial{'s' if n != 1 else ''} at a time")
    return " · ".join(parts)


class Screen:
    """Prints each trial's lines as a block under its name, in its own colour."""

    def __init__(self):
        self.colors: dict[str, str] = {}
        self.last: str | None = None

    def show(self, trial: str, lines: list[str]) -> None:
        if not lines:
            return
        task, _, tag = trial.partition("__")
        color = self.colors.setdefault(trial, COLORS[len(self.colors) % len(COLORS)])
        if OPT.compact:
            for line in lines:
                print(f"{color}[{task}]{RESET} {line}", flush=True)
            return
        if trial != self.last:
            print(f"{color}{BOLD}┏━ {task}{RESET} {DIM}{tag}{RESET}", flush=True)
            self.last = trial
        for line in lines:
            print(f"{color}┃{RESET} {line}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("job", nargs="?", help="a job folder (default: the newest under jobs/, then each newer one)")
    parser.add_argument("--task", action="append", default=[], help="only tasks matching this glob (repeatable)")
    parser.add_argument("--new", action="store_true", help="only what happens from now on")
    parser.add_argument("--from-start", action="store_true", help=argparse.SUPPRESS)  # the default now
    parser.add_argument("--once", action="store_true", help="show what's there and stop")
    parser.add_argument("--thinking", action="store_true", help="the model's whole reasoning")
    parser.add_argument("--full", action="store_true", help="everything, uncut")
    parser.add_argument("--compact", action="store_true", help="one short line per tool call, no output")
    args = parser.parse_args()
    OPT.thinking, OPT.full, OPT.compact = args.thinking, args.full, args.compact

    def wanted(trial: Path) -> bool:
        return not args.task or any(fnmatch.fnmatch(trial.name.split("__")[0], p) for p in args.task)

    follow_newest = args.job is None
    job = Path(args.job) if args.job else newest_job()
    from_start = not args.new
    screen = Screen()
    print(f"{job_line(job)}  {DIM}(Ctrl-C to stop){RESET}\n", flush=True)
    offsets: dict[Path, int] = {}
    pending: dict[Path, str] = {}
    chats: dict[Path, Chat] = {}
    pi_streams: dict[Path, PiStream] = {}
    seen_at: dict[Path, float] = {}  # when each log last had news
    guessed: set[Path] = set()  # logs whose last news came before the watch, when unknown
    news: dict[Path, float] = {}  # when each trial last had news
    beat: dict[Path, float] = {}  # when each trial's last "still working" line was shown
    informed: dict[Path, float] = {}  # trial -> 0 once its header is shown, else when it was last looked for
    rewarded: set[Path] = set()
    started: set[Path] = set()
    failed: set[Path] = set()
    score = [0, 0]
    first_scan = True

    while True:
        # Without a job named: a newer job (the next of a loop) is followed from its start.
        if follow_newest and (newer := newest_job()) != job and newer.stat().st_mtime > job.stat().st_mtime:
            job = newer
            print(f"\n{BOLD}=== next job:{RESET} {job_line(job)}\n", flush=True)
            for state in (offsets, pending, chats, pi_streams, seen_at, news, beat, informed):
                state.clear()
            for state in (rewarded, started, failed, guessed):
                state.clear()
            score = [0, 0]
            screen.last = None
            from_start = True
        now = time.time()
        # What was there before the watch began is shown only from the start (the default).
        past = from_start or not first_scan
        trials = sorted(p for p in job.glob("*__*") if p.is_dir() and wanted(p))
        fresh = [t for t in trials if t not in started]
        started.update(fresh)
        found = {p for pat in PATTERNS for p in job.glob(f"*/agent/{pat}") if wanted(p.parent.parent)}
        # A running trial's logs, readable here or not: read from its container if need be.
        if DOCKER:
            found |= {t / "agent" / n for t in started if not (t / "result.json").exists() for n in LIVE_LOGS}
        by_trial: dict[Path, list[Path]] = {}
        for log in found:
            by_trial.setdefault(log.parent.parent, []).append(log)
        # One trial after another: its header, what its agent did, how it ended, its reward.
        for trial in trials:
            if trial in fresh:
                # One there when the watch began has news as old as its logs say, not from now.
                if not first_scan:
                    news[trial] = now
                if past:
                    screen.show(trial.name, [f"{DIM}trial started{RESET}"])
            if informed.get(trial) != 0 and now - informed.get(trial, 0) >= 5:
                info = trial_info(trial)
                informed[trial] = 0 if info is not None else now
                if info and past:
                    screen.show(trial.name, info)
            # A task's stderr after its other logs: an error usually ends what came before it.
            for log in sorted(by_trial.get(trial, []), key=lambda p: (p.name.endswith(".stderr.txt"), p.name)):
                if log not in offsets:
                    # By default every log is shown from its start; with --new, those there at
                    # start are followed from their end, and a task that starts later from its beginning.
                    offsets[log] = 0 if past else size_of(log)
                new = read_from(log, offsets[log])
                if not new:
                    continue
                offsets[log] += len(new)
                chunk = pending.pop(log, "") + new.decode(errors="replace")
                lines = chunk.split("\n")
                if log.suffix == ".jsonl":
                    if lines[-1]:
                        pending[log] = lines[-1]  # partial JSON line: finish it on the next read
                    lines = lines[:-1]
                is_err = log.name.endswith(".stderr.txt")
                pi = pi_streams.setdefault(log, PiStream()) if log.name == "pi.jsonl" else None
                chat = chats.setdefault(log, Chat("Pi" if pi is not None else "KodAct"))
                # How long the step that just arrived took: since the log's news before it. A
                # log's first read has nothing earlier to count from.
                first = log not in seen_at
                chat.took = None if first else now - seen_at[log]
                chat.took_over = log in guessed
                shown = {e for lg, c in chats.items() if lg.parent.parent == trial for e in c.ended}
                said = [text for line in lines for text in handle(line, is_err, chat, pi, shown)]
                # A trial's header before its first lines: its agent writes it before it starts.
                if said and informed.get(trial) != 0 and (info := trial_info(trial)) is not None:
                    informed[trial] = 0
                    if past:
                        screen.show(trial.name, info)
                screen.show(trial.name, said)
                # A log read for the first time may hold news from before the watch: as old as its
                # last write, from here or from its container. When neither can tell, counted from
                # now and said as "over".
                at = changed_at(log) if first else None
                seen_at[log] = at if at is not None else now
                if first and first_scan and at is None:
                    guessed.add(log)
                else:
                    guessed.discard(log)
                news[trial] = max(news.get(trial, 0), seen_at[log])
            if trial not in failed and (text := trial_error(trial)):
                failed.add(trial)
                screen.show(trial.name, [text])
            reward = trial / "verifier" / "reward.txt"
            if reward not in rewarded and (text := reward_line(reward)):
                rewarded.add(reward)
                score[0] += "passed" in text
                score[1] += 1
                screen.show(trial.name, [f"{text} {DIM}· {score[0]}/{score[1]} passed so far{RESET}"])
        first_scan = False
        if args.once:
            return
        # A sign of life for each trial whose agent is at work with nothing new to show.
        for trial in trials:
            talking = [c for log, c in chats.items() if log.parent.parent == trial and c.events]
            if OPT.compact or not talking or (trial / "result.json").exists():
                continue
            chat = talking[0]
            quiet = now - news.get(trial, now)
            if chat.done or quiet < HEARTBEAT or now - beat.get(trial, 0) < HEARTBEAT:
                continue
            beat[trial] = now
            over = any(log in guessed for log in chats if log.parent.parent == trial)
            screen.show(trial.name, [chat.waiting(quiet, over)])
        time.sleep(1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
