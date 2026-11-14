#!/usr/bin/env python3
"""Write one report file per Harbor job: everything needed to judge and compare a run.

    python3 evals/harbor/report.py                      # the newest job under jobs/
    python3 evals/harbor/report.py jobs/<job> [...]     # given jobs; with two or more, a comparison too
    python3 evals/harbor/report.py --export reports/    # also copy each report there (to commit or send)

Each job gets `jobs/<job>/report.md`: the setup (harness, model, commit, concurrency,
time limits), the score, one row per task with a verdict, the job's signals (API
errors and retries, tool errors by kind, reminders, false successes), and per task
the instruction, how it ended and why, the verifier's failing tests, tool use and
the whole conversation, shortened where tool output is long. A JSON block at the
end holds the same numbers for scripts. With several jobs, a comparison
(`jobs/comparison-<time>.md`) puts the tasks side by side.

Works for KodAct (`agent/kodact.jsonl`), for Pi (`agent/pi.jsonl`, read through
pi_stream.py) and for other agents (Forge Code, Ante, ...), whose `agent/*.stdout.txt`
and `*.stderr.txt` are included as text. API keys are removed before anything is
written. run.sh, run-forgecode.sh, run-ante.sh and run-pi.sh call this script when
their job ends.
"""

import json
import os
import re
import shutil
import sys
import time
from collections import Counter
from datetime import datetime
from pathlib import Path

from pi_stream import PiStream

TEXT = 2000  # characters kept from a message of the model
THINK = 1200  # from one block of reasoning
TOOL_IN = 600  # from a tool call's input
TOOL_OUT = 700  # from a tool's output (head and tail)
INSTRUCTION = 3000
VERIFIER_LINES = 60

SECRET_PATTERNS = [
    re.compile(r"sk-[A-Za-z0-9_\-]{10,}"),
    re.compile(r"(?i)(bearer\s+)[A-Za-z0-9._\-]{12,}"),
    re.compile(r'(?i)((?:api[_-]?key|auth[_-]?token|password)["\']?\s*[:=]\s*["\']?)[^\s"\'*]{8,}'),
]
SECRET_VALUES = [
    os.environ[k]
    for k in ("KODACT_OPENAI_API_KEY", "KODACT_API_KEY", "KODACT_AUTH_TOKEN", "OPENAI_API_KEY", "ANTHROPIC_API_KEY")
    if len(os.environ.get(k, "")) >= 8
]

# The verifier itself failed to run, not the agent's work: its test phase could not reach the
# internet to install packages or download data. On the KPIT endpoint behind a TLS-inspecting
# proxy, the agent trusts the proxy's CA (KODACT_HARBOR_CA_BUNDLE), but pip/uv and huggingface_hub
# in the test phase do not, so the fetch fails with an unknown-issuer / cert-verify error before a
# single assertion about the agent's output runs. A real test failure (a wrong answer) shows an
# AssertionError instead, so these signatures do not match it. Such a trial measures the network,
# not the harness: it is set apart like an API failure, not counted as the agent failing.
VERIFIER_INFRA = re.compile(
    r"(?i)(invalid peer certificate|unknownissuer|certificate[_ ]verify[_ ]failed|unable to get local issuer|"
    r"ssl: certificate|failed to fetch:.*\.(whl|metadata|tar\.gz)|"
    r"couldn.?t be found on the hugging\s*face hub|could not (resolve host|connect to)|"
    r"temporary failure in name resolution|connection refused|max retries exceeded|"
    # A name the resolver couldn't find, as glibc, musl and macOS say it; a URL that couldn't be
    # opened (urllib); a dataset whose every mirror failed (torchvision's MNIST, inside a test).
    r"name or service not known|name does not resolve|nodename nor servname provided|"
    r"network is unreachable|no route to host|urlopen error|error downloading)"
)

# The verifier's own tools could not run in the container, though its tests did: their result
# says nothing about the agent's work. Valgrind under Docker's default file-descriptor limit
# (about a billion) aborts before it starts (custom-memory-heap-crash, 090446d, whose fix passed
# valgrind in the agent's own run once the limit was lowered).
VERIFIER_ENV = re.compile(r"Valgrind: FATAL: Private file creation failed")

# The tests never started because the container's package database was left half-configured:
# their own `apt-get install` refuses to run. That is the agent's doing (it stopped a package
# manager mid-install), so the trial counts, as a failure with its real cause named.
BROKEN_PACKAGES = re.compile(r"dpkg was interrupted, you must manually run")
# The test script's own tools were not there to run (`/tests/test.sh: line 18: uvx: command not
# found`): nothing was judged.
VERIFIER_TOOLS = re.compile(r"(?m)^/tests/\S+: line \d+: \S+: (command not found|No such file or directory)")

REFUSAL = re.compile(r"(?i)\b(i can.?t help with|i cannot help with|i can.?t assist|i won.?t (help|assist)|against (my|the) polic)")
GIVE_UP = re.compile(
    r"(?i)(i.?m sorry|unfortunately|not (realistic|feasible|possible|practical)|can.?t (reliably|realistically)|"
    r"unable to (complete|produce|implement|do)|beyond the scope|too complex)"
)


def redact(text: str) -> str:
    for v in SECRET_VALUES:
        text = text.replace(v, "<REDACTED>")
    for p in SECRET_PATTERNS:
        text = p.sub(lambda m: (m.group(1) if m.groups() else "") + "<REDACTED>", text)
    return text


def clip(text, n: int) -> str:
    text = str(text or "").strip()
    return text if len(text) <= n else text[:n] + f" …[{len(text) - n} more chars]"


def failure_text(test: dict) -> str:
    """A failed test's own error, from json-ctrf: the exception lines of its trace (pytest's `E `
    lines), else its trace or message. Not its source lines, which can say anything."""
    trace = test.get("trace") or ""
    lines = [ln for ln in trace.splitlines() if ln.startswith("E ")]
    return "\n".join(lines) or trace or (test.get("message") or "")


def head_tail(text, n: int) -> str:
    text = str(text or "").strip()
    if len(text) <= n:
        return text
    h = n * 2 // 3
    return f"{text[:h]}\n…[{len(text) - n} chars cut]…\n{text[-(n - h):]}"


def read(path: Path, default=""):
    try:
        return path.read_text(errors="replace")
    except OSError:
        return default


def load_json(path: Path):
    try:
        return json.loads(path.read_text(errors="replace"))
    except (OSError, ValueError):
        return None


def parse_time(s):
    try:
        return datetime.fromisoformat(str(s).replace("Z", "+00:00"))
    except ValueError:
        return None


def seconds(span) -> float | None:
    if not isinstance(span, dict):
        return None
    a, b = parse_time(span.get("started_at")), parse_time(span.get("finished_at"))
    return (b - a).total_seconds() if a and b else None


def fmt_s(s) -> str:
    if s is None:
        return "?"
    s = int(s)
    return f"{s // 60}m{s % 60:02d}s" if s >= 60 else f"{s}s"


def error_kind(text: str) -> str:
    """A failed tool call's message, reduced to its kind: numbers and paths dropped."""
    lines = [ln.strip() for ln in re.sub(r"</?tool_use_error>", "", text).splitlines() if ln.strip()]
    if not lines:
        return "(empty error)"
    first = lines[0]
    if re.fullmatch(r"Exit code \d+", first):
        return "Exit code N"
    first = re.sub(r"(/[\w.\-]+)+", "<path>", first)
    first = re.sub(r"\d+", "N", first)
    return first[:80]


def short_exception(message: str) -> str:
    """Harbor's exception message in one line: what failed, and its stderr when there is one."""
    lines = [ln.strip() for ln in message.strip().splitlines() if ln.strip()]
    if not lines:
        return ""
    m = re.match(r"Command failed \(exit \d+\)", lines[0])
    head = m.group(0) if m else lines[0]
    err = next((ln[7:].strip() for ln in lines if ln.startswith("stderr:") and ln[7:].strip() not in ("", "None")), "")
    return clip(head + (f": {err}" if err else ""), 140)


# KodAct's own reminders (`system` events). Each is either needed or a false positive, so a change
# that makes fewer of them is only better if the pass rate holds.
REMINDERS = (
    "verification",
    "loop_guard",
    "act_reminder",
    "attempt_reminder",
    "next_step_reminder",
    "todo_reminder",
    "background_reminder",
)


def reminder_label(ev: dict) -> str:
    """`loop_guard:repeated_failure after changes`, `verification:unchecked`, `act_reminder`."""
    label = ev.get("subtype", "")
    if ev.get("kind"):
        label += f":{ev['kind']}"
    if ev.get("after_changes"):
        label += " after changes"
    return label


def tool_text(block: dict) -> str:
    body = block.get("content")
    if isinstance(body, list):
        body = "\n".join(b.get("text", "") for b in body if isinstance(b, dict))
    return str(body or "")


def tool_call_text(name: str, args) -> str:
    if not isinstance(args, dict):
        return clip(args, TOOL_IN)
    if name in ("Write", "write") and "content" in args:
        content = str(args.get("content", ""))
        lines = content.splitlines()
        return f"{args.get('file_path') or args.get('path', '')} ({len(lines)} lines)\n{clip(content, TOOL_IN)}"
    if name in ("Bash", "bash"):
        why = f"   # {args['description']}" if args.get("description") else ""
        return clip(args.get("command", ""), TOOL_IN) + why
    return clip(json.dumps(args, ensure_ascii=False), TOOL_IN)


# --------------------------------------------------------------------------- one trial


class Trial:
    def __init__(self, path: Path):
        self.path = path
        self.result = load_json(path / "result.json") or {}
        self.task = self.result.get("task_name") or path.name.split("__")[0]
        rewards = (self.result.get("verifier_result") or {}).get("rewards") or {}
        reward = rewards.get("reward")
        if reward is None:
            try:
                reward = float(read(path / "verifier" / "reward.txt").strip())
            except ValueError:
                reward = None
        self.reward = reward
        exc = self.result.get("exception_info") or {}
        self.exception = exc.get("exception_type") or ""
        self.exception_message = exc.get("exception_message") or ""
        self.agent_s = seconds(self.result.get("agent_execution"))
        self.setup_s = seconds(self.result.get("agent_setup"))
        self.verifier_s = seconds(self.result.get("verifier"))
        m = re.search(r"timed out after ([\d.]+) seconds", self.exception_message)
        self.limit_s = float(m.group(1)) if m else None
        self.run_meta = load_json(path / "agent" / "run.json") or {}
        if not self.limit_s and self.run_meta.get("time_limit_s"):
            self.limit_s = self.run_meta["time_limit_s"]
        self.instruction = read(path / "agent" / "instruction.txt")
        self.events: list[dict] = []
        # A structured stream: KodAct's stream-json, or Pi's events in the same shape.
        pi = (path / "agent" / "pi.jsonl").is_file()
        stream = path / "agent" / ("pi.jsonl" if pi else "kodact.jsonl")
        self.stderr_name = "pi.stderr.txt" if pi else "kodact.stderr.txt"
        self.kodact = stream.is_file()
        translate = PiStream() if pi else None
        if self.kodact:
            for line in read(stream).splitlines():
                try:
                    ev = json.loads(line)
                except ValueError:
                    continue
                if isinstance(ev, dict):
                    self.events.extend(translate.feed(ev) if translate else [ev])
        self.other_logs = {
            p.name: read(p)
            for p in sorted((path / "agent").glob("*"))
            if p.is_file() and p.suffix in (".txt", ".log") and p.name not in ("instruction.txt",)
        }
        self.analyse()

    def analyse(self):
        self.tools = Counter()
        self.tool_errors = Counter()
        self.error_kinds = Counter()
        self.unknown_tools = Counter()
        self.system = Counter()
        self.reminders = Counter()
        self.retries = []
        self.result_event = None
        self.tokens_in = self.tokens_out = 0
        self.final_text = ""
        steps = 0
        names = {}
        for ev in self.events:
            kind = ev.get("type")
            if kind == "assistant":
                msg = ev.get("message") or {}
                usage = msg.get("usage") or {}
                steps += 1
                self.tokens_in += (usage.get("input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
                self.tokens_out += usage.get("output_tokens") or 0
                for b in msg.get("content") or []:
                    if b.get("type") == "tool_use":
                        names[b.get("id")] = b.get("name")
                        self.tools[b.get("name")] += 1
                    elif b.get("type") == "text" and b.get("text", "").strip():
                        self.final_text = b["text"]
            elif kind == "user":
                content = (ev.get("message") or {}).get("content")
                for b in content if isinstance(content, list) else []:
                    if b.get("type") == "tool_result" and b.get("is_error"):
                        text = tool_text(b)
                        name = names.get(b.get("tool_use_id"), "?")
                        self.tool_errors[name] += 1
                        k = error_kind(text)
                        self.error_kinds[k] += 1
                        m = re.search(r"No such tool available: (\S+)", text)
                        if m:
                            self.unknown_tools[m.group(1)] += 1
            elif kind == "system" and ev.get("subtype") not in (None, "init"):
                self.system[ev["subtype"]] += 1
                if ev["subtype"] in REMINDERS:
                    self.reminders[reminder_label(ev)] += 1
                if ev["subtype"] == "api_retry":
                    self.retries.append(ev.get("error", ""))
            elif kind == "result":
                self.result_event = ev
        r = self.result_event or {}
        usage = r.get("usage") or {}
        if usage:
            self.tokens_in = (usage.get("input_tokens") or 0) + (usage.get("cache_read_input_tokens") or 0)
            self.tokens_out = usage.get("output_tokens") or self.tokens_out
        if r.get("result"):
            self.final_text = r["result"]
        # A run that was killed has no result: count its steps instead.
        self.turns = r.get("num_turns", steps if self.kodact else None)
        self.subtype = r.get("subtype") or ("" if not self.kodact else "no result (killed or crashed)")
        self.stop_reason = r.get("stop_reason") or ""
        self.errors = r.get("errors") or []
        stderr = "\n".join(v for k, v in self.other_logs.items() if k.endswith("stderr.txt"))
        self.http_429 = stderr.count("HTTP 429") + sum("429" in e for e in self.retries)
        self.crash_reason = ""
        if not self.kodact:
            out = "\n".join(v for k, v in self.other_logs.items() if not k.endswith("stderr.txt"))
            self.final_text = "\n".join(out.strip().splitlines()[-15:])
            lines = [re.sub(r"^\W*(err(or)?:?\s*)?", "", ln).strip() for ln in stderr.splitlines() if ln.strip()]
            self.crash_reason = clip(lines[-1], 90) if lines else ""
        self.verifier = self.read_verifier()
        self.verdict = self.decide()

    def read_verifier(self) -> dict:
        ctrf = load_json(self.path / "verifier" / "ctrf.json") or {}
        res = ctrf.get("results") or {}
        tests = res.get("tests") or []
        failed = [
            {"name": t.get("name"), "message": clip(t.get("message") or t.get("trace") or "", 600)}
            for t in tests
            if t.get("status") not in ("passed", "skipped")
        ]
        stdout = read(self.path / "verifier" / "test-stdout.txt")
        # The agent did not pass, and the test phase hit a network/cert failure before it could
        # judge the agent's output. Only count it when the tests never ran: pytest prints a
        # "N passed"/"N failed" footer once a test body is reached, so its presence means the tests
        # did evaluate the agent (a wrong answer), even if an unrelated cert warning scrolled past
        # during setup. Its absence, with the infra signature, means the verifier died fetching its
        # own dependencies or data and measured nothing.
        tests_ran = re.search(r"\b\d+ (passed|failed)\b", stdout)
        # Some tests fetch their data in the test body itself (pytorch-model-cli downloads MNIST
        # there), so they ran and failed without judging anything: each failed test's own error is
        # a network failure, and none is an assertion about the agent's output.
        errors = [failure_text(t) for t in tests if t.get("status") not in ("passed", "skipped")]
        failed_fetching = bool(errors) and all(VERIFIER_INFRA.search(e) and "AssertionError" not in e for e in errors)
        self.broken_packages = bool((self.reward or 0) < 1 and not tests_ran and BROKEN_PACKAGES.search(stdout))
        self.verifier_infra = bool(
            (self.reward or 0) < 1
            and not self.broken_packages
            and (
                (not tests_ran and (VERIFIER_INFRA.search(stdout) or VERIFIER_TOOLS.search(stdout)))
                or failed_fetching
                or VERIFIER_ENV.search(stdout)
            )
        )
        return {"summary": res.get("summary") or {}, "failed": failed, "stdout_tail": stdout.splitlines()[-VERIFIER_LINES:]}

    def decide(self) -> str:
        text = self.final_text or ""
        api_error = self.stop_reason == "api_error" or any("API Error" in e for e in self.errors)
        if self.reward is not None and self.reward >= 1:
            return "passed"
        # Out of time: killed by Harbor at the limit, or stopped by KodAct itself (`--max-time`).
        if self.exception == "AgentTimeoutError" or self.stop_reason == "max_time":
            return "timeout"
        if api_error:
            return "api error before any work" if not self.turns else "api error"
        if getattr(self, "broken_packages", False):
            # The agent's work was never judged, but by its own doing: it left dpkg interrupted
            # and the tests could not install what they need. A failure, with its cause.
            return "left dpkg interrupted"
        if getattr(self, "verifier_infra", False) or self.exception == "VerifierTimeoutError":
            # The tests could not judge the agent's work: they failed to fetch what they need,
            # or ran out of time (often the same thing on a slow proxy). Not the agent failing.
            return "verifier error"
        if any("stopped without an answer" in e for e in self.errors):
            # KodAct ended the run as an error: the last replies were all reasoning.
            return "no answer"
        if REFUSAL.search(text[-1500:]) and sum(self.tools.values()) <= 3:
            return "refused"
        if GIVE_UP.search(text[-1500:]) and sum(self.tools.values()) <= 6:
            return "gave up"
        if self.subtype == "success" or (not self.kodact and not self.exception):
            return "false success" if self.kodact else "failed"
        if self.exception:
            if self.crash_reason:
                return f"crashed: {self.crash_reason}"
            why = short_exception(self.exception_message)
            return f"crashed ({self.exception}: {why})" if why else f"crashed ({self.exception})"
        return "failed"

    # ---- output

    def row(self) -> str:
        reward = "?" if self.reward is None else f"{self.reward:g}"
        limit = f"/{fmt_s(self.limit_s)}" if self.limit_s else ""
        tok = f"{self.tokens_in // 1000}k/{self.tokens_out // 1000}k" if self.tokens_in or self.tokens_out else ""
        calls = sum(self.tools.values())
        errs = sum(self.tool_errors.values())
        finish = " ".join(x for x in (self.subtype, self.stop_reason, self.exception) if x)
        return (
            f"| {self.task} | {reward} | {self.verdict} | {self.turns if self.turns is not None else ''} | "
            f"{fmt_s(self.agent_s)}{limit} | {tok} | {calls}/{errs} | {len(self.retries)} | {self.reminder_cell()} | "
            f"{self.compaction_cell()} | {finish} |"
        )

    def compactions(self) -> tuple[int, int, int]:
        """Compactions done, summary attempts that failed, and notes written without the model."""
        return tuple(self.system.get(k, 0) for k in ("compact_boundary", "compact_failed", "compact_fallback"))

    def compaction_cell(self) -> str:
        """`2/7/0`: two compactions, seven failed summaries, no note without the model; empty if none."""
        c = self.compactions()
        return "/".join(map(str, c)) if any(c) else ""

    def reminder_cell(self) -> str:
        """KodAct's reminders by subtype: `verification 1, loop_guard 2`."""
        by = Counter()
        for label, n in self.reminders.items():
            by[label.split(":")[0]] += n
        return ", ".join(f"{k} {v}" for k, v in by.most_common())

    def details(self) -> str:
        out = [f"### {self.task}: {self.verdict}", ""]
        out.append(
            f"- reward {self.reward}, agent {fmt_s(self.agent_s)}"
            + (f" of {fmt_s(self.limit_s)}" if self.limit_s else "")
            + f", setup {fmt_s(self.setup_s)}, verifier {fmt_s(self.verifier_s)}"
        )
        if self.turns is not None:
            out.append(f"- {self.turns} turns, tokens in {self.tokens_in:,} / out {self.tokens_out:,}")
        if self.exception:
            out.append(f"- exception: {self.exception}: {clip(self.exception_message, 300)}")
        if self.kodact:
            out.append(f"- finish: {self.subtype} / {self.stop_reason or '-'}")
        for e in self.errors:
            out.append(f"- error: {clip(e, 400)}")
        if self.system:
            out.append("- harness events: " + ", ".join(f"{k} ×{v}" for k, v in self.system.most_common()))
        if self.reminders:
            out.append("- KodAct reminders: " + ", ".join(f"{k} ×{v}" for k, v in self.reminders.most_common()))
        if self.tools:
            out.append(
                "- tools (calls/errors): "
                + ", ".join(f"{k} {v}/{self.tool_errors.get(k, 0)}" for k, v in self.tools.most_common())
            )
        if self.error_kinds:
            out.append("- tool errors: " + "; ".join(f"{k} ×{v}" for k, v in self.error_kinds.most_common(8)))
        if self.instruction:
            out += ["", "**Instruction**", "", "```text", clip(self.instruction, INSTRUCTION), "```"]
        if self.final_text:
            out += ["", "**Last words of the agent**", "", "```text", clip(self.final_text, TEXT), "```"]
        v = self.verifier
        if v["summary"] or v["failed"]:
            s = v["summary"]
            out += ["", f"**Verifier**: {s.get('passed', '?')} passed, {s.get('failed', '?')} failed of {s.get('tests', '?')}"]
            for f in v["failed"]:
                out.append(f"- `{f['name']}`: {clip(f['message'], 400)}")
            if self.reward is not None and self.reward < 1 and v["stdout_tail"]:
                out += ["", "```text", *v["stdout_tail"], "```"]
        out += ["", "**Conversation**", ""]
        out += self.conversation() if self.kodact else self.plain_logs()
        out.append("")
        return "\n".join(out)

    def conversation(self) -> list[str]:
        out, step, names = ["```text"], 0, {}
        for ev in self.events:
            kind = ev.get("type")
            if kind == "assistant":
                step += 1
                for b in (ev.get("message") or {}).get("content") or []:
                    t = b.get("type")
                    if t == "thinking" and b.get("thinking", "").strip():
                        out.append(f"[{step}] thinks: {clip(b['thinking'], THINK)}")
                    elif t == "text" and b.get("text", "").strip():
                        out.append(f"[{step}] says: {clip(b['text'], TEXT)}")
                    elif t == "tool_use":
                        names[b.get("id")] = b.get("name")
                        out.append(f"[{step}] → {b.get('name')}: {tool_call_text(b.get('name'), b.get('input'))}")
            elif kind == "user":
                content = (ev.get("message") or {}).get("content")
                for b in content if isinstance(content, list) else []:
                    if b.get("type") == "tool_result":
                        mark = "✗" if b.get("is_error") else "←"
                        out.append(f"    {mark} {head_tail(tool_text(b), TOOL_OUT)}")
                    elif b.get("type") == "text" and ev.get("isSynthetic"):
                        out.append(f"    [kodact] {clip(b.get('text', ''), 500)}")
            elif kind == "system" and ev.get("subtype") not in (None, "init"):
                extra = {k: v for k, v in ev.items() if k not in ("type", "subtype", "session_id", "uuid")}
                out.append(f"    [system] {ev['subtype']} {clip(json.dumps(extra, ensure_ascii=False), 300)}")
            elif kind == "result":
                out.append(
                    f"== {ev.get('subtype')} after {ev.get('num_turns')} turns, "
                    f"{(ev.get('duration_ms') or 0) / 1000:.0f}s (API {(ev.get('duration_api_ms') or 0) / 1000:.0f}s)"
                )
        stderr = self.other_logs.get(self.stderr_name, "").strip()
        if stderr:
            counts = Counter(ln.strip() for ln in stderr.splitlines() if ln.strip())
            out += ["", "-- stderr (distinct lines) --"] + [f"{n}× {clip(ln, 300)}" for ln, n in counts.most_common(30)]
        out.append("```")
        return out

    def plain_logs(self) -> list[str]:
        out = []
        for name, text in self.other_logs.items():
            if not text.strip() or name == self.stderr_name:
                continue
            out += [f"`{name}`", "", "```text", head_tail(text, 40_000), "```", ""]
        return out or ["(no agent logs)"]

    def summary(self) -> dict:
        return {
            "task": self.task,
            "reward": self.reward,
            "verdict": self.verdict,
            "exception": self.exception,
            "subtype": self.subtype,
            "stop_reason": self.stop_reason,
            "turns": self.turns,
            "agent_s": self.agent_s,
            "limit_s": self.limit_s,
            "tokens_in": self.tokens_in,
            "tokens_out": self.tokens_out,
            "tool_calls": dict(self.tools),
            "tool_errors": dict(self.tool_errors),
            "error_kinds": dict(self.error_kinds),
            "unknown_tools": dict(self.unknown_tools),
            "system_events": dict(self.system),
            "reminders": dict(self.reminders),
            "api_retries": len(self.retries),
            "http_429": self.http_429,
            "tests": self.verifier["summary"],
            "failed_tests": [f["name"] for f in self.verifier["failed"]],
        }


# --------------------------------------------------------------------------- one job


class Job:
    def __init__(self, path: Path):
        self.path = path
        self.name = path.name
        self.config = load_json(path / "config.json") or {}
        self.result = load_json(path / "result.json") or {}
        self.trials = [Trial(p) for p in sorted(path.iterdir()) if p.is_dir() and (p / "result.json").is_file()]
        agents = self.config.get("agents") or [{}]
        self.agent = agents[0].get("name", "?")
        self.model = (agents[0].get("model_name") or "").split("/", 1)[-1] or (self.trials[0].result.get("config", {}).get("agent", {}).get("model_name") if self.trials else "?")
        self.harness = (
            "KodAct" if "kodact_agent" in self.agent else "Forge Code" if "forgecode" in self.agent
            else "Ante" if "ante" in self.agent else "Pi" if "pi_agent" in self.agent or self.agent == "pi" else self.agent
        )
        self.version = next((t.result.get("agent_info", {}).get("version") for t in self.trials if t.result.get("agent_info")), "?")
        self.concurrency = self.config.get("n_concurrent_trials")
        self.started = self.result.get("started_at")
        self.finished = self.result.get("finished_at")
        meta = next((t.run_meta for t in self.trials if t.run_meta), {})
        self.flags = meta.get("flags")
        # What the endpoint served behind the model's name, and its window (run.sh's probe).
        self.served = meta.get("served_model")
        self.served_window = meta.get("served_window")

    def score(self) -> tuple[int, int]:
        return sum(1 for t in self.trials if (t.reward or 0) >= 1), len(self.trials)

    def report(self) -> str:
        passed, n = self.score()
        verdicts = Counter(t.verdict for t in self.trials)
        invalid = ("api error", "verifier error")
        valid = [t for t in self.trials if not t.verdict.startswith(invalid)]
        wall = None
        if self.started and self.finished:
            a, b = parse_time(self.started), parse_time(self.finished)
            wall = (b - a).total_seconds() if a and b else None
        busy = sum(t.agent_s or 0 for t in self.trials)
        out = [f"# {self.name}", ""]
        out += [
            f"- harness: **{self.harness}** (`{self.agent}`), version `{self.version}`",
            f"- model: `{self.model}`"
            + (f", served by `{self.served}`" if self.served else "")
            + (f" (window {self.served_window} tokens)" if self.served_window else ""),
            f"- tasks in parallel: {self.concurrency}",
            f"- started {self.started}, finished {self.finished}: {fmt_s(wall)} wall clock, "
            f"{fmt_s(busy)} of agent time in total (wall clock includes waiting and any sleep of the machine)",
        ]
        if self.flags:
            out.append(f"- agent flags: `{' '.join(self.flags)}`")
        out += [
            "",
            "## Score",
            "",
            f"- **{passed}/{n} passed** (mean {passed / n if n else 0:.2f})",
            f"- valid trials (model reached, verifier ran): {len(valid)}/{n}; "
            f"passed among them: {sum(1 for t in valid if (t.reward or 0) >= 1)}/{len(valid) or 1}",
            "- verdicts: " + ", ".join(f"{k} {v}" for k, v in verdicts.most_common()),
            "",
            "## Trials",
            "",
            "| task | reward | verdict | turns | agent time | tokens in/out | tool calls/errors | API retries "
            "| KodAct reminders | compactions (done/failed/note) | finish |",
            "|---|---|---|---|---|---|---|---|---|---|---|",
        ]
        out += [t.row() for t in self.trials]
        errors, kinds, unknown, system, reminders = Counter(), Counter(), Counter(), Counter(), Counter()
        for t in self.trials:
            errors.update(t.tool_errors)
            kinds.update(t.error_kinds)
            unknown.update(t.unknown_tools)
            system.update(t.system)
            reminders.update(t.reminders)
        api = Counter(clip(e, 160) for t in self.trials for e in t.errors)
        out += ["", "## Signals", ""]
        out.append(f"- HTTP 429 seen: {sum(t.http_429 for t in self.trials)}; API retries: {sum(len(t.retries) for t in self.trials)}")
        if api:
            out.append("- run-ending errors: " + "; ".join(f"{k} ×{v}" for k, v in api.most_common(6)))
        if kinds:
            out.append("- tool errors by kind: " + "; ".join(f"{k} ×{v}" for k, v in kinds.most_common(12)))
        if errors:
            out.append("- tool errors by tool: " + ", ".join(f"{k} ×{v}" for k, v in errors.most_common()))
        if unknown:
            out.append("- unknown tool names: " + ", ".join(f"{k} ×{v}" for k, v in unknown.most_common()))
        if system:
            out.append("- harness events: " + ", ".join(f"{k} ×{v}" for k, v in system.most_common()))
        if reminders:
            with_any = sum(1 for t in self.trials if t.reminders)
            out.append(
                f"- KodAct reminders: {sum(reminders.values())} in {with_any} of {n} trials: "
                + ", ".join(f"{k} ×{v}" for k, v in reminders.most_common())
            )
        out += ["", "## Tasks in detail", ""]
        out += [t.details() for t in self.trials]
        data = {
            "job": self.name,
            "harness": self.harness,
            "agent": self.agent,
            "version": self.version,
            "model": self.model,
            "concurrency": self.concurrency,
            "started": self.started,
            "finished": self.finished,
            "flags": self.flags,
            "served_model": self.served,
            "served_window": self.served_window,
            "passed": passed,
            "trials": n,
            "valid_trials": len(valid),
            "verdicts": dict(verdicts),
            "per_trial": [t.summary() for t in self.trials],
        }
        out += ["## Data", "", "```json", json.dumps(data, indent=1, ensure_ascii=False), "```", ""]
        return redact("\n".join(out))


def reminders_per_trial(job: Job) -> str:
    """KodAct's reminders per trial, total and by subtype: `0.8 (verification 0.5, loop_guard 0.3)`."""
    n = len(job.trials)
    by = Counter()
    for t in job.trials:
        for label, c in t.reminders.items():
            by[label.split(":")[0]] += c
    if not n or not by:
        return "0"
    parts = ", ".join(f"{k} {v / n:.1f}" for k, v in by.most_common())
    return f"{sum(by.values()) / n:.1f} ({parts})"


def compactions_per_trial(job: Job) -> str:
    """Compactions and failed summary attempts per trial: `0.3/0.5`. Each failed one is a model call
    that bought nothing, so fewer is better as long as the pass rate holds."""
    n = len(job.trials)
    done = sum(t.compactions()[0] for t in job.trials)
    failed = sum(t.compactions()[1] for t in job.trials)
    return f"{done / n:.1f}/{failed / n:.1f}" if n else "0"


def comparison(jobs: list[Job]) -> str:
    tasks = sorted({t.task for j in jobs for t in j.trials})
    out = ["# Comparison", ""]
    out += [
        "| job | harness | model | passed | valid trials | parallel | agent time per pass | KodAct reminders per trial "
        "| compactions/failed summaries per trial | verdicts |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for j in jobs:
        p, n = j.score()
        valid = sum(1 for t in j.trials if not t.verdict.startswith(("api error", "verifier error")))
        v = ", ".join(f"{k} {c}" for k, c in Counter(t.verdict for t in j.trials).most_common())
        # All agent time spent, failed trials included, per task solved: minutes per solved task.
        per_pass = fmt_s(sum(t.agent_s or 0 for t in j.trials) / p) if p else "–"
        out.append(
            f"| {j.name} | {j.harness} | {j.model} | {p}/{n} | {valid} | {j.concurrency} | {per_pass} | "
            f"{reminders_per_trial(j)} | {compactions_per_trial(j)} | {v} |"
        )
    out += ["", "| task | " + " | ".join(f"{j.harness} ({i + 1})" for i, j in enumerate(jobs)) + " |"]
    out.append("|---|" + "---|" * len(jobs))
    for task in tasks:
        cells = []
        for j in jobs:
            t = next((t for t in j.trials if t.task == task), None)
            cells.append("" if t is None else ("✔ " if (t.reward or 0) >= 1 else "") + t.verdict)
        out.append(f"| {task} | " + " | ".join(cells) + " |")
    out += ["", "Columns: " + "; ".join(f"({i + 1}) {j.name}" for i, j in enumerate(jobs)), ""]
    return redact("\n".join(out))


def newest_job() -> Path:
    jobs = [p for p in Path("jobs").glob("*") if p.is_dir() and (p / "config.json").is_file()]
    if not jobs:
        sys.exit("no jobs/ directory with runs here; cd to the KodAct checkout or pass a job path")
    return max(jobs, key=lambda p: p.stat().st_mtime)


def main() -> None:
    args = sys.argv[1:]
    export = None
    if "--export" in args:
        i = args.index("--export")
        if i + 1 >= len(args):
            sys.exit("--export needs a directory")
        export = Path(args[i + 1])
        del args[i : i + 2]
    paths = [Path(a) for a in args] or [newest_job()]
    jobs = []
    for p in paths:
        if not (p / "config.json").is_file():
            print(f"skipping {p}: not a Harbor job folder", file=sys.stderr)
            continue
        job = Job(p)
        jobs.append(job)
        (p / "report.md").write_text(job.report())
        passed, n = job.score()
        print(f"{job.name}: {passed}/{n} passed -> {p / 'report.md'}")
        for t in job.trials:
            print(f"  {t.task:36s} {'?' if t.reward is None else f'{t.reward:g}':>3}  {t.verdict}")
        if export:
            export.mkdir(parents=True, exist_ok=True)
            shutil.copy(p / "report.md", export / f"{job.name}.md")
    if len(jobs) > 1:
        name = f"comparison-{time.strftime('%Y%m%d-%H%M%S')}.md"
        target = (jobs[0].path.parent if not export else export) / name
        target.write_text(comparison(jobs))
        print(f"comparison -> {target}")


if __name__ == "__main__":
    main()
