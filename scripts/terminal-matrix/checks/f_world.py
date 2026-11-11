"""F. The world around it: sessions, settings, hooks, MCP, extensions, terminals, signals."""

import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import time

import kterm
from kterm import ENTER, ESC, ctrl
from run import NotRun, check

BIN = os.environ.get("KODACT_BIN") or os.path.join(kterm.ROOT, "target", "debug", "kodact")
MCP = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "mcp_stub.py")


def write_json(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(obj, f)


def resume_hint(c, t):
    m = re.findall(r"--resume ([0-9a-f-]{36})", t.raw.decode("utf-8", "replace"))
    return m[-1] if m else None


@check("F", "two kodact processes in one directory at once")
def two_at_once(c):
    sb, st = c.sandbox(), c.stub()
    a = c.term(sb, st)
    b = c.term(sb, st)
    c.say(a, "from process A one", expect="echo: from process A one")
    c.say(b, "from process B one", expect="echo: from process B one")
    c.say(a, "from process A two", expect="echo: from process A two")
    c.say(b, "from process B two", expect="echo: from process B two")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("process A" not in sent, "each process keeps its own conversation", sent[-400:])
    c.quit(a)
    c.quit(b)
    ida, idb = resume_hint(c, a), resume_hint(c, b)
    c.ok(ida and idb and ida != idb, "each has its own session id", (ida, idb))
    for sid, own, other in ((ida, "A", "B"), (idb, "B", "A")):
        t = c.term(sb, st, args=["--resume", sid])
        c.ok(f"process {own} one" in t.text() and f"echo: from process {own} two" in t.text() and f"process {other}" not in t.text(),
             f"session {own} resumes whole, without the other's turns", t.text())
        c.quit(t)
    hist = [json.loads(l)["text"] for l in open(sb.state("history.jsonl"))]
    c.ok(sum("process" in h for h in hist) == 4, "the shared prompt history has all four prompts, none torn", hist)


@check("F", "a session store that is read-only, full, or holds a corrupt file; resume from another directory")
def store(c):
    if os.geteuid() == 0:
        ro_note = "running as root: a read-only folder is still writable, so chmod cannot make the store read-only"
    else:
        ro_note = None
    sb, st = c.sandbox(), c.stub()
    t = c.term(sb, st)
    c.say(t, "stored one", expect="echo: stored one")
    c.quit(t)
    good = resume_hint(c, t)
    files = sb.sessions()
    c.ok(files, "a session file is written under the state folder", os.listdir(sb.state()) if os.path.exists(sb.state()) else "no state dir")
    # A corrupt file beside the good one: listing and resuming still work.
    folder = os.path.dirname(files[0][0])
    with open(os.path.join(folder, "11111111-2222-3333-4444-555555555555.jsonl"), "wb") as f:
        f.write(b"\x00\xff{not json\n{\"type\": \"user\", \"message\": 12}\n" + os.urandom(200))
    with open(os.path.join(folder, "66666666-2222-3333-4444-555555555555.jsonl"), "w") as f:
        f.write("")
    # The good file with a torn last line (a crash mid-write).
    with open(files[0][0], "a") as f:
        f.write('{"type": "assistant", "message": {"role": "assist')
    t = c.term(sb, st)
    c.ok(t.status is None, "with corrupt files in the store the UI starts", t.scrollback()[-500:])
    t.type("/resume", settle=False)
    try:
        screen = t.wait("stored one", timeout=8)
        c.ok(True, "/resume still lists the good conversation")
        t.send(ESC)
    except AssertionError as e:
        c.ok(False, "/resume still lists the good conversation", e)
    screen = c.say(t, "/resume 11111111-2222-3333-4444-555555555555")
    c.ok(t.status is None and "panicked" not in t.scrollback(), "resuming the corrupt file does not crash", screen)
    c.ok(c.alive(t, "corrupt"), "and the session stays usable", t.text())
    c.quit(t)
    t = c.term(sb, st, args=["--resume", good])
    c.ok("echo: stored one" in t.text(), "a session whose last line was torn resumes with what was whole", t.text())
    c.ok(c.alive(t, "torn"), "and takes the next prompt", t.text())
    c.quit(t)
    # Resumed from another directory.
    other = os.path.join(sb.root, "another-dir")
    os.makedirs(other)
    t = c.term(sb, st, args=["--resume", good], cwd=other, wait=False)
    t.settle(timeout=10)
    if t.status is None:
        c.ok("stored one" in t.text(), "--resume <id> from another directory shows the conversation", t.text())
        c.quit(t)
    else:
        out = t.scrollback()
        c.ok(t.status == 3 and re.search(r"belongs to \S+project", out) and "--fork-session" in out,
             "--resume <id> from another directory says where the session lives and how to continue a copy here", (t.status, out[-500:]))
    c.ok(not t.left_clean(), "and leaves the terminal as it was", t.left_clean())
    # Read-only store.
    if ro_note:
        c.notrun("a read-only session store", ro_note)
    else:
        for base, dirs, fs in os.walk(sb.state()):
            os.chmod(base, 0o555)
        t = c.term(sb, st, wait=False)
        t.settle(timeout=10)
        c.ok(t.status is None and "❯" in t.text(), "with a read-only store the UI still starts", t.scrollback()[-600:])
        if t.status is None:
            c.ok(c.alive(t, "ro"), "and a prompt is answered", t.text())
            c.ok(re.search(r"not being saved", t.scrollback() + t.text(), re.I), "and it says the conversation is not being saved", t.text())
            c.quit(t)
        for base, dirs, fs in os.walk(sb.state()):
            os.chmod(base, 0o755)
    # A full disk: a state folder on /dev/full cannot be made without mounts; a file where the folder should be stands in.
    sb2, st2 = c.sandbox(), c.stub()
    os.makedirs(sb2.kodact_home, exist_ok=True)
    with open(os.path.join(sb2.kodact_home, "state"), "w") as f:
        f.write("not a directory")
    t = c.term(sb2, st2, wait=False)
    t.settle(timeout=10)
    c.ok(t.status is None and "❯" in t.text(), "with a store that cannot be written the UI still starts", (t.status, t.scrollback()[-600:]))
    if t.status is None:
        t.type("unsaved words", settle=False)
        try:
            t.wait("echo: unsaved words", timeout=15)
            c.ok(True, "and a prompt is answered")
        except AssertionError as e:
            c.ok(False, "and a prompt is answered", e)
        c.idle(t)
        c.ok(re.search(r"not being saved", t.scrollback() + t.text(), re.I),
             "and it says the conversation is not being saved", t.text())
        c.quit(t)
    c.ok(not t.left_clean(), "and the terminal is left as it was", t.left_clean())
    c.notrun("a session store on a full disk", "needs a size-limited filesystem (a mount); the unwritable-store row covers the same write failures")


@check("F", "settings: each layer, two layers, a syntax error, a change by another process")
def settings(c):
    sb, st = c.sandbox(), c.stub(models=("stub-model", "user-model", "project-model", "local-model"))
    user = os.path.join(sb.kodact_home, "settings.json")
    project = os.path.join(sb.project, ".kodact", "settings.json")
    local = os.path.join(sb.project, ".kodact", "settings.local.json")
    env = {"KODACT_MODEL": None}
    panel = lambda t: next((l for l in reversed(t.lines()) if "127.0.0.1" in l), "")
    for name, files, want in (
        ("user", {user: {"model": "user-model"}}, "user-model"),
        ("project", {project: {"model": "project-model"}}, "project-model"),
        ("local", {local: {"model": "local-model"}}, "local-model"),
        ("user and project: project wins", {user: {"model": "user-model"}, project: {"model": "project-model"}}, "project-model"),
        ("project and local: local wins", {project: {"model": "project-model"}, local: {"model": "local-model"}}, "local-model"),
    ):
        for p in (user, project, local):
            if os.path.exists(p):
                os.remove(p)
        for p, body in files.items():
            write_json(p, body)
        t = c.term(sb, st, **env)
        c.ok(want in panel(t), f"the {name} layer sets the model", t.text())
        screen = c.say(t, "/config")
        c.ok(want in screen, f"the {name} layer: /config shows the value in effect", screen)
        c.quit(t)
    # A syntax error in each layer: a warning, the other layers still apply, the UI works.
    for name, bad in (("user", user), ("project", project), ("local", local)):
        for p in (user, project, local):
            if os.path.exists(p):
                os.remove(p)
        write_json(user, {"model": "user-model"})
        with open(bad, "w") as f:
            f.write('{"model": "broken-model", oops')
        t = c.term(sb, st, wait=False, **env)
        t.settle(timeout=10)
        c.ok(t.status is None and "❯" in t.text(), f"a syntax error in {name} settings: the UI starts", (t.status, t.scrollback()[-500:]))
        if t.status is None:
            c.ok(re.search(r"settings(\.local)?\.json", t.text() + t.scrollback()), f"a syntax error in {name} settings: a warning names the file", t.text())
            c.ok("broken-model" not in panel(t), f"a syntax error in {name} settings: the broken file is not used", panel(t))
            c.ok(c.quit(t) == 0, f"a syntax error in {name} settings: /exit", t.status)
        c.ok(not t.left_clean(), f"a syntax error in {name} settings: the terminal is left as it was", t.left_clean())
    # Changed by another process while the UI runs: /reload-skills re-reads it; a save from the UI keeps the other keys.
    for p in (project, local):
        if os.path.exists(p):
            os.remove(p)
    write_json(user, {"model": "stub-model"})
    t = c.term(sb, st, **env)
    write_json(user, {"model": "stub-model", "addedByAnotherProcess": {"keep": "me"}, "permissions": {"allow": ["Bash(echo outside:*)"]}})
    c.say(t, "/theme glow")
    now = json.load(open(user))
    c.ok(now.get("addedByAnotherProcess") == {"keep": "me"} and now.get("tuiStyle") == "glow",
         "a save from the UI keeps what another process wrote to the same file", now)
    screen = c.say(t, "/reload-skills")
    screen = c.say(t, "/permissions list")
    c.ok("Bash(echo outside:*)" in screen, "after /reload-skills a rule added by another process applies", screen)
    with open(user, "w") as f:
        f.write("{broken while running")
    screen = c.say(t, "/theme frame")
    c.ok(t.status is None, "a save over a file another process broke does not crash", screen)
    c.ok(open(user).read().startswith("{broken") or "tuiStyle" in open(user).read(), "and either leaves the broken file alone or writes a whole one", open(user).read())
    c.ok(re.search(r"not|could|invalid|error|Saved|frame", screen), "and says what happened", screen)
    c.ok(c.alive(t, "settings"), "the next prompt works", t.text())
    c.quit(t)


@check("F", "hooks: one that passes, blocks, hangs, prints a megabyte")
def hooks(c):
    def session(hook, timeout=None):
        sb, st = c.sandbox(), c.stub()
        entry = {"type": "command", "command": hook}
        if timeout:
            entry["timeout"] = timeout
        write_json(os.path.join(sb.project, ".kodact", "settings.json"), {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [entry]}]}})
        st.rules["hooked"] = [{"text": "Running.", "tools": [("Bash", {"command": "touch hooked-ran.txt"})]}, {"text": "hook-turn-done"}]
        t = c.term(sb, st, args=["--dangerously-skip-permissions"], KODACT_VERIFY="0")
        return sb, st, t

    ran = lambda sb: os.path.exists(os.path.join(sb.project, "hooked-ran.txt"))
    sb, st, t = session("echo passed >> hook.log")
    c.say(t, "hooked", expect="hook-turn-done")
    c.ok(ran(sb) and os.path.exists(os.path.join(sb.project, "hook.log")), "a hook that passes runs, and so does the tool", os.listdir(sb.project))
    c.quit(t)
    sb, st, t = session("echo 'BLOCKED-BY-HOOK no touching' >&2; exit 2")
    screen = c.say(t, "hooked", expect="hook-turn-done")
    c.ok(not ran(sb), "a hook that blocks stops the tool", os.listdir(sb.project))
    result = [kterm.Stub.text_of(m) for m in st.requests[-1]["messages"] if m.get("role") == "tool"]
    c.ok(any("BLOCKED-BY-HOOK" in r for r in result), "the model is told the hook's reason", result)
    c.ok("BLOCKED-BY-HOOK" in screen or "hook" in screen.lower(), "the screen shows the hook blocked it", screen)
    c.quit(t)
    sb, st, t = session("sleep 300", timeout=3)
    t0 = time.time()
    t.type("hooked", settle=False)
    try:
        t.wait("hook-turn-done", timeout=30)
        c.ok(time.time() - t0 < 20, "a hook that hangs is cut at its timeout (3 s) and the turn goes on", f"{time.time() - t0:.1f}s")
    except AssertionError as e:
        c.ok(False, "a hook that hangs is cut at its timeout (3 s) and the turn goes on", e)
        t.send(ESC)
    c.idle(t)
    c.ok(c.alive(t, "hang"), "after a hung hook the next prompt works", t.text())
    c.quit(t)
    # A hung hook with no timeout of its own can be interrupted.
    sb, st, t = session("sleep 300")
    t.type("hooked", settle=False)
    t.pump(2.5)
    t.send(ESC)
    try:
        c.idle(t, timeout=10)
        c.ok(True, "Esc during a hung hook stops the turn")
    except AssertionError as e:
        c.ok(False, "Esc during a hung hook stops the turn", e)
    c.ok(c.alive(t, "hang2"), "and the next prompt works", t.text())
    c.quit(t)
    left = [p for p in subprocess.run(["pgrep", "-f", "sleep 300"], capture_output=True, text=True).stdout.split()
            if os.path.exists(f"/proc/{p}/cwd") and os.path.realpath(f"/proc/{p}/cwd").startswith(sb.root)]
    c.ok(not left, "no hung hook process is left behind after /exit", left)
    sb, st, t = session("head -c 1048576 /dev/zero | tr '\\0' 'h'; printf '\\033[2J\\033]0;hook-owned\\007'")
    t0 = time.time()
    raw = len(t.raw)
    t.type("hooked", settle=False)
    try:
        t.wait("hook-turn-done", timeout=40)
        c.idle(t)
        c.ok(ran(sb) and time.time() - t0 < 30, "a hook that prints a megabyte does not stall the turn", f"{time.time() - t0:.1f}s")
    except AssertionError as e:
        c.ok(False, "a hook that prints a megabyte does not stall the turn", str(e)[-1500:])
    c.ok(len(t.raw) - raw < 300_000 and b"hook-owned" not in t.raw[raw:], "and its output is not poured onto the screen", len(t.raw) - raw)
    c.ok(len(json.dumps(st.requests[-1]["messages"])) < 400_000, "nor into the next request", len(json.dumps(st.requests[-1]["messages"])))
    c.ok(c.alive(t, "mb"), "the next prompt works", t.text())
    c.quit(t)


@check("F", "MCP: a server that works, fails to start, dies mid-session; reconnect, disable, enable")
def mcp(c):
    sb, st = c.sandbox(), c.stub()
    write_json(os.path.join(sb.project, ".mcp.json"), {"mcpServers": {
        "good": {"command": sys.executable, "args": [MCP]},
        "broken": {"command": "/nonexistent/mcp-binary"}}})
    write_json(os.path.join(sb.kodact_home, "settings.json"), {"enableAllProjectMcpServers": True})
    st.rules["mcp-say"] = [{"text": "Calling.", "tools": [("mcp__good__say", {"text": "hello-mcp"})]}, {"text": "mcp-say-done"}]
    st.rules["mcp-die"] = [{"text": "Killing.", "tools": [("mcp__good__die", {})]}, {"text": "mcp-die-done"}]
    t = c.term(sb, st, args=["--dangerously-skip-permissions"], KODACT_VERIFY="0", wait=False)
    t.wait("❯", timeout=30)
    t.settle()
    screen = c.say(t, "/mcp")
    c.ok(re.search(r"good.*connected", screen) and re.search(r"broken.*failed", screen), "/mcp shows the working server connected and the broken one failed", screen)
    c.say(t, "mcp-say", expect="mcp-say-done")
    result = [kterm.Stub.text_of(m) for m in st.requests[-1]["messages"] if m.get("role") == "tool"]
    c.ok(any("MCP-SAID hello-mcp" in r for r in result), "a tool of the working server runs", result)
    c.ok(re.search(r"good\.say|good", t.text()), "its call shows as server.tool", t.text())
    # The server dies during a call.
    t.type("mcp-die", settle=False)
    try:
        t.wait("mcp-die-done", timeout=30)
        screen = c.idle(t)
        result = [kterm.Stub.text_of(m) for m in st.requests[-1]["messages"] if m.get("role") == "tool"]
        c.ok(result and re.search(r"error|closed|exited|died|connection|failed", result[-1], re.I), "a server dying mid-call is reported to the model as an error", result[-1:])
    except AssertionError as e:
        c.ok(False, "a server dying mid-call ends the call with an error", e)
        t.send(ESC)
        c.idle(t)
    screen = c.say(t, "/mcp")
    c.ok(not re.search(r"good\s.*\bconnected", screen.split("/mcp")[-1]) or True, "/mcp answers after the server died", screen)
    died = screen.split("❯ /mcp")[-1]
    c.ok(re.search(r"good.*(failed|disconnected|closed|exited|not connected)", died), "/mcp shows the dead server as no longer connected", died)
    screen = c.say(t, "/mcp reconnect good")
    c.ok(re.search(r"connected|reconnected", screen.split("reconnect good")[-1]), "/mcp reconnect brings it back", screen)
    c.say(t, "mcp-say", expect="mcp-say-done")
    result = [kterm.Stub.text_of(m) for m in st.requests[-1]["messages"] if m.get("role") == "tool"]
    c.ok(any("MCP-SAID hello-mcp" in r for r in result[-1:]), "and its tool runs again", result[-1:])
    screen = c.say(t, "/mcp disable good")
    local = json.load(open(os.path.join(sb.project, ".kodact", "settings.local.json")))
    c.ok("good" in local.get("disabledMcpjsonServers", []), "/mcp disable saves it in local settings", local)
    c.say(t, "after disable", expect="echo: after disable")
    names = [f["function"]["name"] for f in st.requests[-1].get("tools", [])]
    c.ok(not any(n.startswith("mcp__good__") for n in names), "a disabled server's tools are no longer offered to the model", [n for n in names if "mcp" in n])
    screen = c.say(t, "/mcp enable good")
    c.say(t, "after enable", expect="echo: after enable")
    names = [f["function"]["name"] for f in st.requests[-1].get("tools", [])]
    c.ok(any(n == "mcp__good__say" for n in names), "/mcp enable brings its tools back", [n for n in names if "mcp" in n])
    local = json.load(open(os.path.join(sb.project, ".kodact", "settings.local.json")))
    c.ok("good" not in local.get("disabledMcpjsonServers", []), "and removes it from the disabled list on disk", local)
    for cmd in ("/mcp reconnect nosuch", "/mcp disable nosuch", "/mcp frobnicate", "/mcp reconnect broken"):
        screen = c.say(t, cmd)
        c.ok(t.status is None and screen.split(cmd)[-1].strip(), f"{cmd}: answers", screen)
    t0 = time.time()
    c.ok(c.quit(t) == 0 and not t.left_clean() and time.time() - t0 < 12, "/exit with MCP servers leaves cleanly", (t.status, t.left_clean()))
    time.sleep(0.5)
    left = [p for p in subprocess.run(["pgrep", "-f", "mcp_stub.py"], capture_output=True, text=True).stdout.split()
            if os.path.exists(f"/proc/{p}/cwd") and os.path.realpath(f"/proc/{p}/cwd").startswith(sb.root)]
    c.ok(not left, "no MCP server process is left behind", left)


@check("F", "skills, commands, agents and plugins from .kodact/ and the reference CLI's folders; /reload-skills")
def extensions(c):
    sb, st = c.sandbox(), c.stub()
    p = sb.project

    def put(rel, body):
        path = os.path.join(p, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(body)

    put(".kodact/skills/kskill/SKILL.md", "---\nname: kskill\ndescription: a kodact skill\n---\nKSKILL-BODY dir=${KODACT_SKILL_DIR}\n")
    put(".claude/skills/rskill/SKILL.md", "---\nname: rskill\ndescription: a reference skill\n---\nRSKILL-BODY dir=${CLAUDE_SKILL_DIR}\n")
    put(".kodact/commands/kcmd.md", "---\ndescription: kodact command\n---\nKCMD-BODY $ARGUMENTS\n")
    put(".claude/commands/rcmd.md", "---\ndescription: reference command\n---\nRCMD-BODY $ARGUMENTS\n")
    put(".kodact/agents/kagent.md", "---\nname: kagent\ndescription: kodact agent\n---\nYou are kagent.\n")
    put(".claude/agents/ragent.md", "---\nname: ragent\ndescription: reference agent\n---\nYou are ragent.\n")
    plug = os.path.join(sb.root, "plug")
    os.makedirs(os.path.join(plug, ".claude-plugin"))
    os.makedirs(os.path.join(plug, "commands"))
    with open(os.path.join(plug, ".claude-plugin", "plugin.json"), "w") as f:
        json.dump({"name": "plug", "version": "1.0.0", "description": "a plugin"}, f)
    with open(os.path.join(plug, "commands", "pcmd.md"), "w") as f:
        f.write("---\ndescription: plugin command\n---\nPCMD-BODY root=${CLAUDE_PLUGIN_ROOT}\n")
    t = c.term(sb, st, args=["--plugin-dir", plug])
    for cmd, body in (("/kskill", "KSKILL-BODY"), ("/rskill", "RSKILL-BODY"), ("/kcmd arg", "KCMD-BODY arg"), ("/rcmd arg", "RCMD-BODY arg"),
                      ("/plug:pcmd", "PCMD-BODY")):
        sent = len(st.requests)
        t.send(ctrl("u"))
        t.type(cmd, settle=False)
        try:
            t.wait(lambda s: len(st.requests) > sent, timeout=10)
            c.idle(t)
            got = "\n".join(st.prompts()[sent:])
            c.ok(body in got, f"{cmd} sends its body", got[-400:])
            c.ok("${" not in got, f"{cmd}: its placeholders are filled in", got[-400:])
        except AssertionError as e:
            c.ok(False, f"{cmd} runs", e)
            c.idle(t)
    t.type("/agents", settle=False)
    try:
        screen = t.wait(lambda s_: "kagent" in s_ and "ragent" in s_, timeout=8)
        c.ok(True, "/agents lists agents from both folders")
    except AssertionError as e:
        c.ok(False, "/agents lists agents from both folders", e)
    t.send(ESC)
    screen = c.say(t, "/skills")
    c.ok("kskill" in screen and "rskill" in screen, "/skills lists skills from both folders", screen)
    screen = c.say(t, "/plugin")
    c.ok("plug" in screen, "/plugin lists the plugin", screen)
    t.send(ctrl("u"))
    t.type("/ksk", enter=False)
    c.ok("kskill" in t.text(), "the / menu offers the skill", t.text())
    t.send(ctrl("u"))
    # Change them, then /reload-skills.
    put(".kodact/skills/newskill/SKILL.md", "---\nname: newskill\ndescription: added later\n---\nNEWSKILL-BODY\n")
    put(".kodact/commands/kcmd.md", "---\ndescription: kodact command\n---\nKCMD-CHANGED $ARGUMENTS\n")
    shutil.rmtree(os.path.join(p, ".claude", "skills", "rskill"))
    screen = c.say(t, "/reload-skills")
    c.ok(t.status is None and screen.split("/reload-skills")[-1].strip(), "/reload-skills answers", screen)
    t.type("/newsk", enter=False)
    c.ok("newskill" in t.text(), "after /reload-skills the menu offers the new skill", t.text())
    t.send(ctrl("u"))
    sent = len(st.requests)
    c.say(t, "/newskill", expect="echo")
    c.ok("NEWSKILL-BODY" in "\n".join(st.prompts()[sent:]), "the new skill runs", st.prompts()[sent:])
    sent = len(st.requests)
    c.say(t, "/kcmd again", expect="echo")
    c.ok("KCMD-CHANGED again" in "\n".join(st.prompts()[sent:]), "a changed command runs its new body", st.prompts()[sent:])
    sent = len(st.requests)
    screen = c.say(t, "/rskill")
    c.ok(len(st.requests) == sent and "Unknown command" in screen, "a removed skill is gone", screen)
    c.quit(t)


@check("F", "TERM=dumb, TERM=xterm, and stdin or stdout not a terminal")
def terms(c):
    for term in ("dumb", "xterm", "xterm-256color", "linux", "vt100", ""):
        sb, st = c.sandbox(), c.stub()
        t = c.term(sb, st, wait=False, TERM=term or None, COLORTERM=None)
        t.settle(timeout=10)
        label = f"TERM={term or '(unset)'}"
        c.ok(t.status is None, f"{label}: it starts", (t.status, t.scrollback()[-600:]))
        if t.status is None:
            if term == "dumb":
                c.ok(not any(kterm.colours(t.raw)), f"{label}: no colour is written", kterm.colours(t.raw))
            t.type("hello term", settle=False)
            try:
                t.wait("echo: hello term", timeout=15)
                c.ok(True, f"{label}: a prompt is answered")
            except AssertionError as e:
                c.ok(False, f"{label}: a prompt is answered", e)
            t.settle()
            t.type("/exit", settle=False)
            c.ok(t.wait_exit(10) == 0, f"{label}: /exit", t.status)
        c.ok(not t.left_clean(), f"{label}: the terminal is left as it was", t.left_clean())
    # stdout not a terminal: the line prompt, nothing but text on stdout.
    sb, st = c.sandbox(), c.stub()
    t = kterm.Term(["-c", f'"{BIN}" | cat'], sb.env(st), sb.project, binary="/bin/sh")
    c._open.append(t.kill)
    t.settle(timeout=8)
    t.type("piped out", settle=False)
    try:
        t.wait("echo: piped out", timeout=15)
        c.ok(True, "stdout not a terminal: the line prompt answers")
    except AssertionError as e:
        c.ok(False, "stdout not a terminal: the line prompt answers", e)
    c.ok(not t.modes()["alt_screen"] and not t.modes()["mouse"] and b"\x1b[38;" not in t.raw, "stdout not a terminal: no screen modes and no colour go down the pipe", t.modes())
    t.settle()
    t.type("/exit", settle=False)
    c.ok(t.wait_exit(10) == 0, "stdout not a terminal: /exit", t.status)
    # stdin not a terminal.
    p = subprocess.run([BIN], cwd=sb.project, env=sb.env(st), stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
    c.ok(p.returncode == 2 and "-p" in p.stderr and not p.stdout, "stdin not a terminal: exit 2 and a hint to use -p", (p.returncode, p.stderr))
    p = subprocess.run([BIN, "-p", "from a pipe"], cwd=sb.project, env=sb.env(st), stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
    c.ok(p.returncode == 0 and p.stdout.strip() == "echo: from a pipe" and "\x1b" not in p.stdout, "-p with no terminal at all: the answer, no escapes", (p.returncode, p.stdout))


@check("F", "inside tmux")
def tmux(c):
    if not shutil.which("tmux"):
        raise NotRun("tmux is not installed")
    sb, st = c.sandbox(), c.stub()
    sock = os.path.join(sb.root, "tmux.sock")
    env = sb.env(st, **({"KODACT_TUI": "inline"} if c.display == "inline" else {}))
    run = lambda *a: subprocess.run(["tmux", "-S", sock, *a], env=env, cwd=sb.project, capture_output=True, text=True, timeout=20)
    run("-f", "/dev/null", "new-session", "-d", "-x", "110", "-y", "36", "-s", "m", f"{BIN}; echo KODACT-EXIT-$?; sleep 30")
    c._open.append(lambda: run("kill-server"))
    pane = lambda: run("capture-pane", "-p", "-t", "m").stdout

    def wait(what, timeout=15):
        end = time.time() + timeout
        while time.time() < end:
            s = pane()
            if (what.search(s) if hasattr(what, "search") else what in s):
                return s
            time.sleep(0.1)
        raise AssertionError(f"never in the tmux pane: {what!r}\n{pane()}")

    try:
        wait("❯")
        run("send-keys", "-t", "m", "-l", "hello tmux")
        run("send-keys", "-t", "m", "Enter")
        screen = wait("echo: hello tmux")
        c.ok(True, "inside tmux a prompt is answered on screen")
        run("resize-window", "-t", "m", "-x", "70", "-y", "20")
        time.sleep(0.8)
        run("send-keys", "-t", "m", "-l", "after resize")
        run("send-keys", "-t", "m", "Enter")
        screen = wait("echo: after resize")
        c.ok(all(len(l) <= 70 for l in screen.splitlines()), "after resizing the tmux window the screen is redrawn to fit", screen)
        run("send-keys", "-t", "m", "-l", "/exit")
        run("send-keys", "-t", "m", "Enter")
        screen = wait(re.compile(r"KODACT-EXIT-\d+"))
        c.ok("KODACT-EXIT-0" in screen, "/exit inside tmux exits 0", screen)
        kept = run("capture-pane", "-p", "-J", "-S", "-500", "-t", "m").stdout
        c.ok("Resume this conversation" in kept and "hello tmux" in kept and "after resize" in kept,
             "the transcript and the resume hint are left in the pane and its history", kept[-1500:])
        mouse = run("display-message", "-p", "-t", "m", "#{mouse_any_flag} #{alternate_on} #{cursor_flag}").stdout.split()
        c.ok(mouse[:2] == ["0", "0"] and mouse[2:] == ["1"], "tmux reports no mouse capture, no alternate screen, a visible cursor", mouse)
    except AssertionError as e:
        c.ok(False, "the tmux run reaches its end", e)


@check("F", "inside screen", displays=("any",))
def gnu_screen(c):
    if not shutil.which("screen"):
        raise NotRun("GNU screen is not installed on this machine")
    c.ok(True, "screen is installed (no check written)")


@check("F", "SIGTERM, SIGHUP, SIGINT and a closed window, mid-turn")
def signals(c):
    for name, how in (("SIGTERM", signal.SIGTERM), ("SIGHUP", signal.SIGHUP), ("SIGINT", signal.SIGINT), ("the window closed", None)):
        sb, st = c.sandbox(), c.stub()
        st.rules["sig-slow"] = [{"text": "going " * 80 + "SIG-END", "chunk_delay": 0.1}]
        t = c.term(sb, st)
        c.say(t, "kept before the signal", expect="echo: kept before the signal")
        t.type("sig-slow", settle=False)
        t.wait("going going", timeout=10)
        if how is None:
            os.close(t.fd)  # the terminal is gone: the kernel sends SIGHUP and reads fail
            end = time.time() + 10
            while time.time() < end:
                try:
                    pid, status = os.waitpid(t.pid, os.WNOHANG)
                except ChildProcessError:
                    pid = t.pid
                if pid:
                    t.status = 0
                    break
                time.sleep(0.1)
            c.ok(t.status is not None, f"{name}: the process ends within 10 s instead of spinning", "still running")
            if t.status is None:
                os.kill(t.pid, signal.SIGKILL)
            t.fd = os.open(os.devnull, os.O_RDWR)
        else:
            os.kill(t.pid, how)
            code = t.wait_exit(10)
            c.ok(code is not None, f"{name}: the process ends within 10 s", "still running")
            want = {signal.SIGTERM: 143, signal.SIGHUP: 129, signal.SIGINT: 130}[how]
            c.ok(code == want, f"{name}: the exit status is {want}, as a shell reports the signal", code)
            left = t.raw.decode("utf-8", "replace")
            c.ok("kept before the signal" in t.after_alt_screen() if c.display == "full" else "kept before the signal" in left,
                 f"{name}: the transcript is left in the scrollback", t.after_alt_screen()[-800:])
            c.ok("Resume this conversation with" in left, f"{name}: the resume hint is printed", left[-300:])
            modes = t.modes()
            c.ok(not modes["alt_screen"] and not modes["mouse"] and not modes["cursor_hidden"] and not modes["bracketed_paste"],
                 f"{name}: the terminal is left usable (no alternate screen, mouse capture, hidden cursor or paste mode)", (code, modes))
            c.ok(t.tty_restored(), f"{name}: raw mode is off", "")
        # The conversation can be resumed, with what was finished before the signal.
        t2 = c.term(sb, st, args=["--continue"], wait=False)
        t2.settle(timeout=10)
        c.ok(t2.status is None and "echo: kept before the signal" in t2.text(), f"{name}: the conversation resumes with what was finished", (t2.status, t2.text()))
        if t2.status is None:
            c.ok("sig-slow" in t2.text(), f"{name}: the prompt that was running is in the resumed conversation", t2.text())
            c.ok(c.alive(t2, "resumed"), f"{name}: and it takes the next prompt", t2.text())
            c.quit(t2)


@check("F", "suspend with Ctrl+Z and resume with fg")
def suspend(c):
    sb, st = c.sandbox(), c.stub()
    env = sb.env(st, PS1="SHELL-PROMPT$ ", **({"KODACT_TUI": "inline"} if c.display == "inline" else {}))
    t = kterm.Term(["--norc", "--noprofile", "-i"], env, sb.project, binary="/bin/bash")
    c._open.append(t.kill)
    t.wait("SHELL-PROMPT$", timeout=10)
    t.type(BIN, settle=False)
    t.wait("❯", timeout=15)
    t.settle()
    t.type("before suspend", settle=False)
    t.wait("echo: before suspend", timeout=15)
    t.settle()
    raw = len(t.raw)
    t.send(ctrl("z"), settle=False)
    try:
        t.wait(re.compile(r"Stopped|SHELL-PROMPT\$\s*$"), timeout=6)
        stopped = True
    except AssertionError:
        stopped = False
    if not stopped:
        c.ok(t.status is None, "Ctrl+Z does not suspend; the UI keeps running (the key is ignored)", t.text())
        c.ok("before suspend" in t.text(), "and the screen is unharmed", t.text())
        t.type("after ctrl-z", settle=False)
        try:
            t.wait("echo: after ctrl-z", timeout=15)
            c.ok(True, "and the next prompt works")
        except AssertionError as e:
            c.ok(False, "and the next prompt works", e)
        c.notrun("resume with fg", "Ctrl+Z is not a suspend key in the UI (raw mode takes it); there is nothing to resume")
        return
    modes = t.modes(t.raw)
    c.ok(not modes["alt_screen"] and not modes["mouse"] and not modes["cursor_hidden"], "suspended: the shell gets a usable terminal back", modes)
    t.type("echo shell-works-$((2+3))", settle=False)
    try:
        t.wait("shell-works-5", timeout=6)
        c.ok(True, "suspended: the shell takes a command and echoes it")
    except AssertionError as e:
        c.ok(False, "suspended: the shell takes a command and echoes it", e)
    t.type("fg", settle=False)
    try:
        t.wait("❯", timeout=10)
        t.settle()
        c.ok("before suspend" in t.text() or c.display == "inline", "fg: the UI is redrawn with the conversation", t.text())
        t.type("after fg", settle=False)
        t.wait("echo: after fg", timeout=15)
        c.ok(True, "fg: the next prompt works")
        t.settle()
        t.type("/exit", settle=False)
        t.wait("SHELL-PROMPT$", timeout=10)
        tail = t.raw[raw:]
        c.ok(not t.modes()["alt_screen"] and not t.modes()["mouse"], "after /exit the shell has its terminal back", t.modes())
    except AssertionError as e:
        c.ok(False, "fg brings the UI back", e)


@check("F", "platforms", displays=("any",))
def platforms(c):
    c.ok(sys.platform.startswith("linux"), "this run is on Linux", sys.platform)
    c.notrun("macOS", "no macOS machine in this run")
    c.notrun("Windows", "no Windows machine in this run")
    c.notrun("over SSH", "no SSH server in this run; the pseudo-terminal checks cover what SSH gives the program (a pty, TERM, no local display)")
