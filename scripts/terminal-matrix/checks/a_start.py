"""A. Start and first run."""

import json
import os
import re
import socket
import subprocess

import kterm
from kterm import ESC, Sandbox, Stub
from run import SIZES, check

BIN = os.environ.get("KODACT_BIN") or os.path.join(kterm.ROOT, "target", "debug", "kodact")


def print_mode(sb, stub, args, stdin=None, **env):
    return subprocess.run([BIN, *args], cwd=sb.project, env=sb.env(stub, **env), capture_output=True, text=True,
                          stdin=subprocess.DEVNULL if stdin is None else None, input=stdin, timeout=60)


@check("A", "no endpoint: the first-run card")
def no_endpoint(c):
    sb = c.sandbox()
    # No KODACT_HOME: the default folders, so "nothing outside the config and state folders" is checked for real.
    before = sb.files()
    t = c.term(sb, None, wait=False, KODACT_HOME=None)
    screen = t.settle(timeout=8)
    c.ok("Set up an endpoint" in screen, "the card shows", screen)
    c.ok("KODACT_OPENAI_BASE_URL" in screen and "KODACT_API_KEY" in screen, "it names what is missing", screen)
    t.send("x", settle=False)
    code = t.wait_exit(5)
    c.ok(code == 3, "a key closes it with exit status 3 (configuration missing)", f"exit {code}")
    c.ok(not t.left_clean(), "the terminal is left as it was", t.left_clean())
    new = {f for f in sb.files() - before}
    outside = {f for f in new if not re.match(r"home/\.(config|state|cache)/kodact/", f)}
    c.ok(not outside, "nothing is written outside the config, state and cache folders", outside)
    c.ok(not any(f.startswith("project/") for f in new), "nothing is written in the project", new)


@check("A", "provider paths start and answer", displays=("full",))
def providers(c):
    for name, env in [
        ("KODACT_OPENAI_BASE_URL", lambda st: {"KODACT_OPENAI_BASE_URL": st.url, "KODACT_OPENAI_API_KEY": "k-openai-123456"}),
        ("KODACT_BASE_URL (Messages API)", lambda st: {"KODACT_BASE_URL": st.url[:-3], "KODACT_API_KEY": "k-messages-123456"}),
        ("Gemini (KODACT_GEMINI_BASE_URL)", lambda st: {"KODACT_GEMINI_BASE_URL": st.url + "beta", "KODACT_GEMINI_API_KEY": "k-gemini-123456"}),
    ]:
        sb, st = c.sandbox(), c.stub()
        t = c.term(sb, None, KODACT_MODEL="stub-model", KODACT_CONTEXT_WINDOW=200000, **env(st))
        try:
            screen = c.say(t, "hello provider", expect="echo: hello provider")
            c.ok(True, f"{name}: a prompt is answered on screen")
        except AssertionError as e:
            c.ok(False, f"{name}: a prompt is answered on screen", e)
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{name}: /exit leaves cleanly", (t.status, t.left_clean()))


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@check("A", "a wrong key, URL, host or model gives a message to act on")
def bad_endpoint(c):
    cases = [
        ("a wrong key", dict(key="right-key-123456"), lambda st: {"KODACT_OPENAI_API_KEY": "wrong-key-654321"},
         r"401|key|KODACT_OPENAI_API_KEY"),
        ("a wrong URL path", {}, lambda st: {"KODACT_OPENAI_BASE_URL": st.url + "/nowhere"}, r"404|KODACT_OPENAI_BASE_URL|not found"),
        ("an unreachable host", {}, lambda st: {"KODACT_OPENAI_BASE_URL": f"http://127.0.0.1:{free_port()}/v1", "KODACT_MAX_RETRIES": "0"},
         r"connect|KODACT_OPENAI_BASE_URL"),
        ("a model the endpoint does not list", {}, lambda st: {"KODACT_MODEL": "no-such-model"}, r"no-such-model|/model|404"),
    ]
    for name, stub_kw, env, expect in cases:
        sb, st = c.sandbox(), c.stub(**stub_kw)
        t = c.term(sb, st, **env(st))
        t.type("hello", settle=False)
        try:
            screen = t.wait(re.compile(expect, re.I), timeout=30)
            screen = c.idle(t, timeout=40)
            c.ok(True, f"{name}: the screen says what failed")
        except AssertionError as e:
            screen = t.text()
            c.ok(False, f"{name}: the screen says what failed", e)
        c.ok("wrong-key-654321" not in screen, f"{name}: the key is not printed", screen)
        # The UI stays usable: a local command still answers, and typing still shows.
        t.send(kterm.ctrl("u"))
        t.type("/status", settle=False)
        try:
            t.wait("Session:", timeout=10)
            c.ok(True, f"{name}: the UI stays usable (/status answers)")
        except AssertionError as e:
            c.ok(False, f"{name}: the UI stays usable (/status answers)", e)
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{name}: /exit leaves cleanly", (t.status, t.left_clean()))


@check("A", "kodact \"prompt\" sends it at start")
def prompt_arg(c):
    sb, st = c.sandbox(), c.stub()
    t = c.term(sb, st, args=["first words"], wait=False)
    try:
        t.wait("echo: first words", timeout=20)
        c.ok(True, "the prompt and its answer are on screen")
    except AssertionError as e:
        c.ok(False, "the prompt and its answer are on screen", e)
    c.ok(c.alive(t, "a"), "the next prompt works", t.text())
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("A", "-p prints the answer and nothing else", displays=("any",))
def dash_p(c):
    sb, st = c.sandbox(), c.stub()
    p = print_mode(sb, st, ["-p", "hello print"])
    c.ok(p.returncode == 0 and p.stdout.strip() == "echo: hello print", "stdout is the answer", (p.returncode, p.stdout, p.stderr))
    p = print_mode(sb, st, ["-p", "--output-format", "json", "hello json"])
    try:
        doc = json.loads(p.stdout)
        c.ok(doc.get("result") == "echo: hello json" and doc.get("exit_code") == 0, "json is one document", p.stdout)
    except ValueError:
        c.ok(False, "json is one document", p.stdout)
    p = print_mode(sb, st, ["-p", "from arg"], stdin="from stdin")
    c.ok("from stdin" in p.stdout and "from arg" in p.stdout, "stdin and the prompt are both sent", p.stdout)
    p = print_mode(sb, st, [])
    c.ok(p.returncode == 2 and "terminal" in p.stderr, "no terminal on stdin: exit 2 and a hint", (p.returncode, p.stderr))


def conversation(c, sb, st, words):
    """One conversation with two turns; returns its session id."""
    t = c.term(sb, st)
    for w in words:
        c.say(t, w, expect=f"echo: {w}")
    c.quit(t)
    m = re.search(r"--resume ([0-9a-f-]{36})", t.after_alt_screen() if c.display == "full" else t.scrollback())
    return m.group(1) if m else None


@check("A", "--continue and --resume show the earlier conversation")
def resume_flags(c):
    sb, st = c.sandbox(), c.stub()
    sid = conversation(c, sb, st, ["alpha one", "alpha two"])
    c.ok(sid, "leaving prints the resume hint with the session id", sid)
    for args, name in ([["--continue"], "--continue"], [["--resume", sid or "x"], "--resume <id>"], [["-c"], "-c"], [["-r", sid or "x"], "-r <id>"]):
        t = c.term(sb, st, args=args)
        screen = t.text()
        c.ok("alpha one" in screen and "echo: alpha two" in screen, f"{name}: the earlier conversation is on screen", screen)
        before = len(st.requests)
        c.say(t, "alpha three", expect="echo: alpha three")
        sent = json.dumps(st.requests[-1]["messages"]) if len(st.requests) > before else ""
        c.ok("alpha one" in sent and "alpha two" in sent, f"{name}: the next prompt goes to that conversation", sent[-500:])
        c.ok(c.quit(t) == 0, f"{name}: /exit", t.status)
        c.ok(sid in (t.after_alt_screen() if c.display == "full" else t.scrollback()), f"{name}: the hint names the same session", "")
    t = c.term(sb, st, args=["--resume", "00000000-0000-0000-0000-000000000000"], wait=False)
    code = t.wait_exit(10)
    c.ok(code == 3, "--resume with an unknown id exits 3", (code, t.text()))
    c.ok(re.search(r"session|conversation", t.scrollback(), re.I), "and says the session does not exist", t.scrollback())
    c.ok(not t.left_clean(), "and leaves the terminal as it was", t.left_clean())
    # --resume with no id: a picker of this folder's conversations.
    t = c.term(sb, st, args=["--resume"], wait=False)
    try:
        screen = t.wait("alpha one", timeout=10)
        t.send(kterm.ENTER)
        screen = t.wait("echo: alpha two", timeout=10)
        c.ok(True, "--resume with no id offers the conversations and opens the chosen one")
    except AssertionError as e:
        c.ok(False, "--resume with no id offers the conversations and opens the chosen one", e)
    c.quit(t)
    c.ok(not t.left_clean(), "and leaves the terminal as it was after /exit", t.left_clean())


@check("A", "--no-tui and KODACT_TUI=0 use the line prompt", displays=("any",))
def no_tui(c):
    for name, args, env in (("--no-tui", ["--no-tui"], {}), ("KODACT_TUI=0", [], {"KODACT_TUI": "0"})):
        sb, st = c.sandbox(), c.stub()
        t = kterm.Term(args, sb.env(st, **env), sb.project)
        c._open.append(t.kill)
        t.settle(timeout=8)
        c.ok(not t.modes()["alt_screen"] and not t.modes()["mouse"], f"{name}: no alternate screen, no mouse capture", t.modes())
        t.type("hello line", settle=False)
        try:
            t.wait("echo: hello line", timeout=15)
            c.ok(True, f"{name}: a prompt is answered")
        except AssertionError as e:
            c.ok(False, f"{name}: a prompt is answered", e)
        t.settle()
        t.type("/exit", settle=False)
        c.ok(t.wait_exit(10) == 0 and not t.left_clean(), f"{name}: /exit leaves cleanly", (t.status, t.left_clean()))


@check("A", "--model and --permission-mode show in the panel")
def model_and_mode(c):
    sb, st = c.sandbox(), c.stub(models=("stub-model", "other-model"))
    t = c.term(sb, st, args=["--model", "other-model"])
    c.ok("other-model" in t.text(), "--model: the panel names it", t.text())
    c.say(t, "hi", expect="echo: hi")
    c.ok(st.requests[-1]["model"] == "other-model", "--model: the request uses it", st.requests[-1]["model"])
    c.quit(t)
    names = {"default": "Default", "acceptEdits": "Accept edits", "plan": "Plan", "dontAsk": "Don't ask",
             "bypassPermissions": "Bypass", "auto": "Auto"}
    for mode, label in names.items():
        t = c.term(sb, st, args=["--permission-mode", mode])
        screen = t.text()
        c.ok(label.lower() in screen.lower(), f"--permission-mode {mode}: the panel says {label}", screen)
        t.type("/status", settle=False)
        try:
            t.wait(re.compile(rf"{mode}", re.I), timeout=8)
            c.ok(True, f"--permission-mode {mode}: /status agrees")
        except AssertionError as e:
            c.ok(False, f"--permission-mode {mode}: /status agrees", e)
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"--permission-mode {mode}: /exit leaves cleanly", (t.status, t.left_clean()))
    t = c.term(sb, st, args=["--permission-mode", "nonsense"], wait=False)
    code = t.wait_exit(8)
    c.ok(code == 2, "--permission-mode with a wrong value is a usage error (exit 2)", (code, t.scrollback()[-400:]))


@check("A", "--add-dir and --no-input")
def add_dir_no_input(c):
    sb, st = c.sandbox(), c.stub()
    extra = os.path.join(sb.root, "extra")
    os.makedirs(extra)
    t = c.term(sb, st, args=["--add-dir", extra])
    t.type("/permissions list", settle=False)
    try:
        t.wait("extra", timeout=8)
        c.ok(True, "--add-dir: the directory is a working directory (/permissions)")
    except AssertionError as e:
        c.ok(False, "--add-dir: the directory is a working directory (/permissions)", e)
    c.quit(t)
    t = c.term(sb, st, args=["--no-input"], wait=False)
    code = t.wait_exit(8)
    c.ok(code == 2, "--no-input: interactive mode refuses to start (exit 2)", (code, t.scrollback()[-300:]))
    c.ok(not t.left_clean(), "--no-input: the terminal is left as it was", t.left_clean())


STYLES = ["frame", "glow", "card", "ledger", "status", "bare"]


def front_ok(c, t, label, typed="zq"):
    """What a person needs from the front page at any size: an input they can see and type in."""
    if t.status is not None:
        return c.ok(False, f"{label}: the program is still running", f"exit {t.status}\n{t.scrollback()[-1500:]}")
    t.send(typed)
    screen = t.settle()
    rows = [i for i, l in enumerate(t.lines()) if typed in l]
    ok = c.ok(rows, f"{label}: what is typed shows", screen)
    if rows:
        x, y = t.cursor()
        line = t.lines()[y] if y < len(t.lines()) else ""
        at = line.find(typed)
        c.ok(y in rows and x == wcw(line[:at + len(typed)]), f"{label}: the cursor is where the next character lands",
             f"cursor {(x, y)}, text on rows {rows}\n{screen}")
    for _ in typed:
        t.send(kterm.BS, settle=False)
    t.settle()
    return ok


def wcw(s):
    import unicodedata
    return sum(0 if unicodedata.combining(ch) else 2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in s)


@check("A", "front page: every style at every size, and after a resize")
def front_sizes(c):
    for style in STYLES:
        for cols, rows in SIZES:
            sb, st = c.sandbox(), c.stub()
            with open(os.path.join(sb.kodact_home, "settings.json"), "w") as f:
                json.dump({"tuiStyle": style}, f)
            t = c.term(sb, st, cols=cols, rows=rows, wait=False)
            t.settle(timeout=10)
            label = f"{style} {cols}x{rows}"
            front_ok(c, t, label)
            # Resize while it is on screen: to the smallest, then to the largest.
            for nc, nr in ((40, 12), (200, 50)):
                if t.status is None:
                    t.resize(nc, nr)
                    front_ok(c, t, f"{label} resized to {nc}x{nr}")
            if t.status is None:
                c.ok(c.quit(t) == 0 and not t.left_clean(), f"{label}: /exit leaves cleanly", (t.status, t.left_clean()))
            for f in (t.kill, st.close, sb.close):
                f()


@check("A", "front page: colours, colour depths, NO_COLOR, earlier conversations, a failed MCP server")
def front_colours(c):
    raw_colour = lambda t: kterm.colours(t.raw)

    for theme in ("dark", "light", "none", "auto"):
        sb, st = c.sandbox(), c.stub()
        with open(os.path.join(sb.kodact_home, "settings.json"), "w") as f:
            json.dump({"theme": theme}, f)
        t = c.term(sb, st)
        front_ok(c, t, f"theme {theme}")
        if theme == "none":
            c.ok(not any(raw_colour(t)), "theme none: no colour escapes are written", raw_colour(t))
        c.quit(t)
    for depth, idx in (("truecolor", 0), ("256", 1), ("16", 2)):
        sb, st = c.sandbox(), c.stub()
        t = c.term(sb, st, KODACT_COLOR_DEPTH=depth)
        front_ok(c, t, f"depth {depth}")
        tc, c256, c16 = raw_colour(t)
        c.ok(raw_colour(t)[idx], f"depth {depth}: colours of that depth are written", raw_colour(t))
        if depth != "truecolor":
            c.ok(not tc, f"depth {depth}: no 24-bit colour is written", "")
        if depth == "16":
            c.ok(not c256, "depth 16: no 256-colour is written", "")
        c.quit(t)
    sb, st = c.sandbox(), c.stub()
    t = c.term(sb, st, NO_COLOR="1")
    front_ok(c, t, "NO_COLOR")
    c.ok(not any(raw_colour(t)), "NO_COLOR: no colour escapes are written", raw_colour(t))
    c.say(t, "remember me", expect="echo: remember me")
    c.quit(t)
    for cols, rows in SIZES:
        t = c.term(sb, st, cols=cols, rows=rows, wait=False)
        screen = t.settle(timeout=10)
        if cols >= 60:
            c.ok("remember me" in screen and "/resume" in screen, f"an earlier conversation is offered at {cols}x{rows}", screen)
        front_ok(c, t, f"with an earlier conversation {cols}x{rows}")
        c.quit(t)
    # An MCP server that fails to start.
    with open(os.path.join(sb.project, ".mcp.json"), "w") as f:
        json.dump({"mcpServers": {"brokenserver": {"command": "/nonexistent/mcp-server"}}}, f)
    with open(os.path.join(sb.kodact_home, "settings.json"), "w") as f:
        json.dump({"enableAllProjectMcpServers": True}, f)
    for cols, rows in SIZES:
        t = c.term(sb, st, cols=cols, rows=rows, wait=False)
        screen = t.settle(timeout=12)
        if cols >= 80 and rows >= 24:
            c.ok("brokenserver" in screen, f"a failed MCP server is named at {cols}x{rows}", screen)
        front_ok(c, t, f"with a failed MCP server {cols}x{rows}")
        c.quit(t)
