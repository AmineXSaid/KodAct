"""B (continued). Commands that last (files on disk, held after a restart, undone) and
commands that change the conversation (the screen, the meter, the next prompt, the way back)."""

import json
import os
import re

import kterm
from kterm import DOWN, ENTER, ESC, PGDN, PGUP, UP, ctrl
from run import check


def read(path):
    try:
        return json.load(open(path))
    except (OSError, ValueError):
        return {}


def user_settings(sb):
    return read(os.path.join(sb.kodact_home, "settings.json"))


def local_settings(sb):
    return read(os.path.join(sb.project, ".kodact", "settings.local.json"))


def session_id(c, t):
    t.send(ctrl("u"))
    screen = c.say(t, "/status", expect="Session:")
    return re.findall(r"Session:\s+([0-9a-f-]{36})", screen)[-1]


def answer(c, t, text, expect=None):
    """Run a local command and return the screen."""
    t.send(ctrl("u"))
    return c.say(t, text, expect=expect)


@check("B", "settings that last: on disk, held after a restart, undone")
def lasting(c):
    sb, st = c.sandbox(), c.stub(models=("stub-model", "other-model"))
    extra = os.path.join(sb.root, "extra-dir")
    os.makedirs(extra)
    st.rules["run-echo"] = [{"text": "Running.", "tools": [("Bash", {"command": "touch allowed-by-rule.txt"})]}, {"text": "ran it."}]
    import shutil
    have_sandbox = bool(shutil.which("bwrap"))
    t = c.term(sb, st)
    c.say(t, "seed", expect="echo: seed")
    steps = [
        ("/theme glow", lambda: user_settings(sb).get("tuiStyle") == "glow", "tuiStyle is glow in user settings"),
        ("/theme light", lambda: user_settings(sb).get("theme") == "light", "theme is light in user settings"),
        ("/model other-model", lambda: user_settings(sb).get("model") == "other-model", "model is saved in user settings"),
        ("/output-style explanatory", lambda: local_settings(sb).get("outputStyle") == "explanatory", "outputStyle is saved in local settings"),
        ("/autocompact off", lambda: user_settings(sb).get("autoCompactEnabled") is False, "autoCompactEnabled is false in user settings"),
        ("/sandbox read-only", lambda: local_settings(sb).get("sandbox", {}).get("mode") == "read-only", "sandbox.mode is saved in local settings"),
        ("/permissions add allow Bash(touch:*)", lambda: "Bash(touch:*)" in local_settings(sb).get("permissions", {}).get("allow", []),
         "the allow rule is in local settings"),
        ("/hooks add PreToolUse Bash echo hook-ran >> hook.log", lambda: "hook-ran" in json.dumps(local_settings(sb).get("hooks", {})),
         "the hook is in local settings"),
        ("/statusline echo STATUSLINE-MARK", lambda: user_settings(sb).get("statusLine", {}).get("command") == "echo STATUSLINE-MARK",
         "statusLine is saved in user settings"),
        (f"/add-dir {extra} --save", lambda: extra in json.dumps(local_settings(sb)), "the directory is saved in local settings"),
        ("/config verification.enabled=false", lambda: user_settings(sb).get("verification", {}).get("enabled") is False,
         "verification.enabled is false in user settings"),
        ("/agents create helper --description helps-with-things", lambda: os.path.exists(os.path.join(sb.project, ".kodact", "agents", "helper.md")),
         "the agent file is written"),
    ]
    if not have_sandbox:
        steps = [s_ for s_ in steps if not s_[0].startswith("/sandbox")]
        c.notrun("/sandbox read-only saved and held", "no sandbox backend (bwrap) on this machine: /sandbox refuses to turn on, and says so")
    for cmd, on_disk, what in steps:
        screen = answer(c, t, cmd)
        c.ok(on_disk(), f"{cmd}: {what}", (screen, user_settings(sb), local_settings(sb)))
    c.ok("STATUSLINE-MARK" in t.text(), "/statusline: the status line shows the command's output", t.text())
    c.quit(t)

    # Start again: each one held. (No KODACT_MODEL: the environment would win over the saved model.)
    t = c.term(sb, st, KODACT_MODEL=None)
    front = t.text()
    c.ok("other-model" in front, "after a restart: the model held (the panel names it)", front)
    c.ok(kterm.colours(t.raw)[0] or kterm.colours(t.raw)[1], "after a restart: the light theme draws in colour", "")
    c.say(t, "seed two", expect="echo: seed two")
    c.ok(st.requests[-1]["model"] == "other-model", "after a restart: requests use the saved model", st.requests[-1]["model"])
    c.ok("STATUSLINE-MARK" in t.text(), "after a restart: the status line command held", t.text())
    t.type("/theme", settle=False)
    screen = t.wait("Choose a style", timeout=8)
    c.ok(re.search(r"glow ✔", screen) and re.search(r"light colours ✔", screen), "after a restart: /theme marks glow and light", screen)
    t.send(ESC)
    t.type("/output-style", settle=False)
    screen = t.wait("Select an output style", timeout=8)
    c.ok(re.search(r"explanatory ✔", screen), "after a restart: /output-style marks explanatory", screen)
    t.send(ESC)
    screen = answer(c, t, "/autocompact")
    c.ok(re.search(r"\boff\b", screen), "after a restart: /autocompact says off", screen)
    if have_sandbox:
        screen = answer(c, t, "/sandbox")
        c.ok("read-only" in screen, "after a restart: /sandbox says read-only", screen)
    screen = answer(c, t, "/permissions list")
    c.ok("Bash(touch:*)" in screen and "extra-dir" in screen, "after a restart: /permissions lists the rule and the directory", screen)
    screen = answer(c, t, "/hooks list")
    answer(c, t, "/sandbox off")
    t.type("run-echo", settle=False)
    try:
        t.wait("ran it.", timeout=15)
        c.idle(t)
        c.ok(os.path.exists(os.path.join(sb.project, "allowed-by-rule.txt")) and "Allow once" not in t.text(),
             "after a restart: the allow rule lets the command run unasked", t.text())
    except AssertionError as e:
        c.ok(False, "after a restart: the allow rule lets the command run unasked", e)
    log = os.path.join(sb.project, "hook.log")
    c.ok(os.path.exists(log) and "hook-ran" in open(log).read(), "after a restart: the saved hook ran before the tool", os.listdir(sb.project))
    t.type("/agents", settle=False)
    try:
        t.wait("helper", timeout=8)
        c.ok(True, "after a restart: /agents lists the created agent")
    except AssertionError as e:
        c.ok(False, "after a restart: /agents lists the created agent", e)
    t.send(ESC)

    # Undo each, and confirm that too.
    undo = [
        ("/theme frame", lambda: user_settings(sb).get("tuiStyle") == "frame"),
        ("/theme auto", lambda: user_settings(sb).get("theme") in ("auto", None)),
        ("/model stub-model", lambda: user_settings(sb).get("model") == "stub-model"),
        ("/output-style default", lambda: local_settings(sb).get("outputStyle") in ("default", None)),
        ("/autocompact on", lambda: user_settings(sb).get("autoCompactEnabled") in (True, None)),
        ("/permissions remove Bash(touch:*)", lambda: "Bash(touch:*)" not in json.dumps(local_settings(sb).get("permissions", {}).get("allow", []))),
        ("/hooks remove PreToolUse 1", lambda: "hook-ran" not in json.dumps(local_settings(sb).get("hooks", {}))),
        ("/statusline off", lambda: not user_settings(sb).get("statusLine", {}).get("command")),
        ("/config verification.enabled=", lambda: "verification" not in user_settings(sb) or "enabled" not in user_settings(sb)["verification"]),
    ]
    for cmd, gone in undo:
        screen = answer(c, t, cmd)
        c.ok(gone(), f"{cmd}: undone on disk", (screen, user_settings(sb), local_settings(sb)))
    c.ok("STATUSLINE-MARK" not in "\n".join(t.lines()[-3:]), "/statusline off: the status line no longer shows it", t.text())
    os.remove(log)
    t.type("run-echo", settle=False)
    try:
        t.wait("Allow once", timeout=15)
        c.ok(True, "after removing the rule the command asks again")
        t.send("y")
        t.wait("ran it.", timeout=15)
        c.idle(t)
    except AssertionError as e:
        c.ok(False, "after removing the rule the command asks again", e)
    c.ok(not os.path.exists(log), "after removing the hook it no longer runs", "")
    c.quit(t)
    t = c.term(sb, st, KODACT_MODEL=None)
    c.ok("stub-model" in t.text() and "other-model" not in t.text(), "after another restart the undone model held", t.text())
    c.ok("STATUSLINE-MARK" not in t.text(), "after another restart the status line command is gone", t.text())
    c.quit(t)


@check("B", "pickers: choose, filter, page, cancel")
def pickers(c):
    sb, st = c.sandbox(), c.stub(models=["stub-model"] + [f"model-{i:02d}" for i in range(40)])
    # Earlier conversations, for /resume.
    for words in (["pear one", "pear two"], ["plum one"], ["fig one"]):
        t = c.term(sb, st)
        for w in words:
            c.say(t, w, expect=f"echo: {w}")
        c.quit(t)
    t = c.term(sb, st)
    c.say(t, "current one", expect="echo: current one")
    sid = session_id(c, t)

    # /model: 41 rows, so it pages.
    t.type("/model", settle=False)
    screen = t.wait("Select a model", timeout=8)
    c.ok("stub-model ✔" in screen, "/model: the current model is marked", screen)
    t.send(PGDN)
    t.send(PGDN)
    paged = t.text()
    c.ok(paged != screen and "Select a model" in paged, "/model: PgDn pages through the rows", paged)
    t.send(PGUP)
    c.ok(t.text() != paged and "Select a model" in t.text(), "/model: PgUp pages back", t.text())
    t.send(kterm.HOME)
    c.ok(re.search(r"❯ 1\. ", t.text()) and "Select a model" in t.text(), "/model: Home goes to the first row, the title still in view", t.text())
    t.send(kterm.END)
    c.ok(re.search(r"❯ 42\. Another model", t.text()) and "Select a model" in t.text() and "type to filter" in t.text(),
         "/model: End reaches the last of 42 rows with the title and filter still in view", t.text())
    t.send(kterm.HOME)
    t.send("model-27")
    filtered = t.text()
    c.ok("model-27" in filtered and "model-03" not in filtered, "/model: typing filters the rows", filtered)
    t.send(ESC)
    c.ok("Select a model" not in t.text(), "/model: Esc closes the picker", t.text())
    c.say(t, "still stub", expect="echo: still stub")
    c.ok(st.requests[-1]["model"] == "stub-model", "/model: Esc left the model as it was", st.requests[-1]["model"])
    t.type("/model", settle=False)
    t.wait("Select a model", timeout=8)
    t.send("model-27")
    t.send(ENTER)
    c.idle(t)
    c.say(t, "now 27", expect="echo: now 27")
    c.ok(st.requests[-1]["model"] == "model-27", "/model: Enter on a filtered row switches to it", st.requests[-1]["model"])
    answer(c, t, "/model stub-model")
    # "Another model…" puts the command in the input to finish by hand.
    t.type("/model", settle=False)
    t.wait("Select a model", timeout=8)
    t.send("Another")
    t.send(ENTER)
    c.ok(t.input_text().startswith("/model"), "/model: 'Another model…' leaves /model in the input to finish", t.text())
    t.send(ctrl("u"))

    # /resume: rows for the other conversations; Esc leaves this one.
    t.type("/resume", settle=False)
    try:
        screen = t.wait("pear one", timeout=8)
        c.ok("plum one" in screen and "fig one" in screen, "/resume: lists the other conversations by their first prompt", screen)
        t.send("plum")
        c.ok("plum one" in t.text() and "fig one" not in t.text(), "/resume: typing filters", t.text())
        t.send(ESC)
        c.ok("pear one" not in t.text() and session_id(c, t) == sid, "/resume: Esc leaves the conversation as it was", t.text())
        t.type("/resume", settle=False)
        t.wait("pear one", timeout=8)
        t.send("pear")
        t.send(ENTER)
        c.idle(t)
        screen = t.text()
        c.ok("echo: pear two" in screen and "current one" not in screen, "/resume: Enter shows the chosen conversation, not the one left", screen)
        c.say(t, "pear three", expect="echo: pear three")
        sent = json.dumps(st.requests[-1]["messages"])
        c.ok("pear one" in sent and "current one" not in sent, "/resume: the next prompt goes to the chosen conversation", sent[-600:])
    except AssertionError as e:
        c.ok(False, "/resume: the picker works", e)

    # /theme, /output-style, /permissions: Esc changes nothing.
    before = (user_settings(sb), local_settings(sb))
    for cmd, title in (("/theme", "Choose a style"), ("/output-style", "Select an output style"), ("/permissions", "Permission rules")):
        t.send(ctrl("u"))
        t.type(cmd, settle=False)
        try:
            t.wait(title, timeout=8)
            t.send(DOWN)
            t.send(DOWN)
            t.send(UP)
            t.send(ESC)
            c.ok(title not in t.text(), f"{cmd}: arrows move and Esc closes", t.text())
        except AssertionError as e:
            c.ok(False, f"{cmd}: the picker opens", e)
    c.ok((user_settings(sb), local_settings(sb)) == before, "Esc in the pickers changed no settings file", (before, user_settings(sb), local_settings(sb)))
    t.type("/theme", settle=False)
    t.wait("Choose a style", timeout=8)
    t.send("ledger")
    t.send(ENTER)
    c.idle(t)
    c.ok(user_settings(sb).get("tuiStyle") == "ledger", "/theme: choosing a row saves it", user_settings(sb))
    c.ok(c.alive(t, "ledger"), "/theme: the UI works in the new style", t.text())
    # Screens: /diff, /context, /hooks, /agents open, scroll and close.
    with open(os.path.join(sb.project, "notes.txt"), "a") as f:
        f.write("a changed line\n" * 60)
    for cmd, expect in (("/diff", "notes.txt"), ("/context", "System prompt"), ("/hooks", "Add hook"), ("/agents", "Create an agent")):
        t.send(ctrl("u"))
        t.type(cmd, settle=False)
        try:
            screen = t.wait(expect, timeout=10)
            t.send(PGDN)
            t.send(DOWN)
            t.send(PGUP)
            t.send(kterm.END)
            t.send(kterm.HOME)
            c.ok(t.status is None and expect in t.text(), f"{cmd}: the screen opens and scrolls", t.text())
            t.send(ESC)
            t.send(ESC)
            c.ok(not c.dialog(t.text()), f"{cmd}: Esc closes it", t.text())
        except AssertionError as e:
            c.ok(False, f"{cmd}: the screen opens", e)
    c.ok(c.alive(t, "screens"), "after the screens the next prompt works", t.text())
    c.quit(t)


def tokens_used(screen):
    m = re.findall(r"([\d.]+k?) tokens used", screen)
    return m[-1] if m else None


@check("B", "/clear, /reset, /new: the screen, the meter, the next prompt, the way back")
def clear(c):
    for cmd in ("/clear", "/reset", "/new", "/clear named-one"):
        sb, st, t = c.session()
        c.say(t, "apple first", expect="echo: apple first")
        c.say(t, "apple second", expect="echo: apple second")
        old = session_id(c, t)
        screen = answer(c, t, cmd, expect="Conversation cleared")
        c.ok("apple first" not in screen.replace("/resume  apple first", "") and "echo: apple second" not in screen,
             f"{cmd}: the old conversation is off the screen", screen)
        if c.display == "inline":
            c.ok("echo: apple second" not in t.scrollback(), f"{cmd}: and out of the window's scrollback", t.scrollback()[-1500:])
        c.ok(tokens_used(screen) is None or "100% left" in screen or re.search(r"9[6-9]% left", screen), f"{cmd}: the context meter starts over", screen)
        new = session_id(c, t)
        c.ok(new != old, f"{cmd}: the session has a new id", (old, new))
        c.say(t, "banana first", expect="echo: banana first")
        sent = json.dumps(st.requests[-1]["messages"])
        c.ok("apple" not in sent, f"{cmd}: the next prompt goes to an empty conversation", sent[-600:])
        screen = answer(c, t, f"/resume {old}")
        c.ok("apple first" in screen and "echo: apple second" in screen and "banana first" not in screen,
             f"{cmd}: /resume <old id> shows the conversation left behind, whole", screen)
        c.say(t, "apple third", expect="echo: apple third")
        sent = json.dumps(st.requests[-1]["messages"])
        c.ok("apple first" in sent and "apple second" in sent and "banana" not in sent, f"{cmd}: and the next prompt goes to it", sent[-600:])
        if "named" in cmd:
            screen = answer(c, t, "/resume named-one")
            c.ok("apple first" in screen or "already" in screen.lower(), f"{cmd}: the name given finds it", screen)
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{cmd}: /exit leaves cleanly", (t.status, t.left_clean()))


@check("B", "/rewind in each mode: files, conversation and screen go back together")
def rewind(c):
    for action in ("both", "conversation", "code", "summarize-from", "summarize-to"):
        sb, st = c.sandbox(), c.stub()
        made = os.path.join(sb.project, "made.txt")
        notes = os.path.join(sb.project, "notes.txt")
        st.rules["make-files"] = [
            {"text": "Editing.", "tools": [("Write", {"file_path": made, "content": "made by the turn\n"})]},
            {"text": "Reading.", "tools": [("Read", {"file_path": notes})]},
            {"text": "More.", "tools": [("Edit", {"file_path": notes, "old_string": "first line", "new_string": "CHANGED line"})]},
            {"text": "files made."}]
        t = c.term(sb, st, args=["--permission-mode", "acceptEdits"], KODACT_VERIFY="0")
        c.say(t, "cherry one", expect="echo: cherry one")
        t.type("make-files", settle=False)
        t.wait("files made.", timeout=20)
        c.idle(t)
        c.say(t, "cherry three", expect="echo: cherry three")
        c.ok(os.path.exists(made) and "CHANGED" in open(notes).read(), f"{action}: the turn changed the files (setup)", os.listdir(sb.project))
        screen = answer(c, t, f"/rewind 2 {action}")
        files_back = not os.path.exists(made) and "first line" in open(notes).read()
        if action in ("both", "code"):
            c.ok(files_back, f"{action}: the files are as before prompt 2", (os.listdir(sb.project), open(notes).read()))
        else:
            c.ok(os.path.exists(made) and "CHANGED" in open(notes).read(), f"{action}: the files stay as they are", os.listdir(sb.project))
        t.send(ctrl("u"))
        screen = t.settle()
        if action in ("both", "conversation"):
            c.ok("echo: cherry one" in screen and "files made." not in screen and "echo: cherry three" not in screen,
                 f"{action}: the screen shows the conversation up to prompt 2 only", screen)
        elif action == "code":
            c.ok("files made." in screen and "echo: cherry three" in screen, f"{action}: the conversation stays on screen", screen)
        c.say(t, "after rewind", expect="after rewind")
        sent = json.dumps(st.requests[-1]["messages"])
        if action in ("both", "conversation"):
            c.ok("cherry one" in sent and "cherry three" not in sent and "files made" not in sent,
                 f"{action}: the next prompt goes to the rewound conversation", sent[-800:])
        elif action == "code":
            c.ok("cherry three" in sent, f"{action}: the next prompt still has the whole conversation", sent[-800:])
        elif action == "summarize-from":
            c.ok("cherry one" in sent and "cherry three" not in sent and "SUMMARY-MARK" in sent,
                 f"{action}: prompt 2 onward is replaced by a summary", sent[-1200:])
        elif action == "summarize-to":
            c.ok("SUMMARY-MARK" in sent and "cherry three" in sent, f"{action}: what came before prompt 2 is a summary, the rest stays", sent[-1200:])
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{action}: /exit leaves cleanly", (t.status, t.left_clean()))
    # The picker: Esc leaves everything; a wrong number says so.
    sb, st, t = c.session()
    c.say(t, "date one", expect="echo: date one")
    c.say(t, "date two", expect="echo: date two")
    t.type("/rewind", settle=False)
    t.wait("Rewind to before which prompt", timeout=8)
    t.send(ESC)
    c.say(t, "date three", expect="echo: date three")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("date one" in sent and "date two" in sent, "the picker: Esc leaves the conversation whole", sent[-500:])
    screen = answer(c, t, "/rewind 99 both")
    c.ok(re.search(r"no prompt|only|between|1 to|invalid|isn't|not", screen, re.I) and "date three" in screen, "a prompt number out of range says so and changes nothing", screen)
    t.send(ESC)
    t.send(ESC)
    screen = t.settle()
    c.ok("Rewind to before which prompt" in screen, "Esc twice on an empty prompt opens the rewind picker", screen)
    t.send(ESC)
    c.quit(t)


@check("B", "/branch: work in each branch, /resume between them")
def branch(c):
    sb, st, t = c.session()
    c.say(t, "grape one", expect="echo: grape one")
    original = session_id(c, t)
    screen = answer(c, t, "/branch side-road")
    c.ok("echo: grape one" in screen or "grape one" in t.scrollback(), "/branch: the copied conversation is on screen", screen)
    side = session_id(c, t)
    c.ok(side != original, "/branch: the session has a new id", (original, side, screen))
    c.say(t, "grape side", expect="echo: grape side")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("grape one" in sent, "/branch: the branch continues from the original's history", sent[-500:])
    screen = answer(c, t, f"/resume {original}")
    c.ok("echo: grape one" in screen and "grape side" not in screen, "/resume <original>: its screen has no trace of the branch's work", screen)
    c.say(t, "grape main", expect="echo: grape main")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("grape one" in sent and "grape side" not in sent, "the original's next prompt does not see the branch", sent[-500:])
    screen = answer(c, t, f"/resume {side}")
    c.ok("echo: grape side" in screen and "grape main" not in screen, "/resume <branch>: back in the branch, whole", screen)
    screen = answer(c, t, "/resume side-road")
    c.ok("already" in screen.lower() or "grape side" in screen, "/resume by the branch's name finds it", screen)
    c.quit(t)


@check("B", "/compact: by hand, with instructions, on an empty conversation")
def compact(c):
    sb, st, t = c.session()
    screen = answer(c, t, "/compact")
    c.ok(re.search(r"nothing to compact", screen, re.I), "/compact on an empty conversation says there is nothing to compact", screen)
    c.ok(len(st.requests) == 0, "and makes no model call", len(st.requests))
    c.say(t, "the secret number is 4172", expect="4172")
    c.say(t, "kiwi two", expect="echo: kiwi two")
    screen = answer(c, t, "/compact", expect="ompacted")
    c.ok("ompacted" in screen, "/compact says it compacted", screen)
    c.say(t, "kiwi three", expect="kiwi three")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("SUMMARY-MARK" in sent and "the secret number is 4172" in sent,
         "the next request carries the summary and the person's requests word for word", sent[-1500:])
    c.ok("echo: kiwi two" not in sent, "and the old answers are no longer sent", sent[-800:])
    before = len(st.requests)
    answer(c, t, "/compact keep the kiwi facts", expect="ompacted")
    asked = [r for r in st.requests[before:] if "is about to be replaced by a summary" in json.dumps(r["messages"][-1])]
    c.ok(asked and "keep the kiwi facts" in json.dumps(asked[0]["messages"][-1]), "/compact <instructions>: the instructions reach the summary request",
         json.dumps(st.requests[-1]["messages"][-1])[-800:])
    c.ok(c.alive(t, "compact"), "after compacting the next prompt works", t.text())
    c.quit(t)


@check("B", "/cd: the session moves, the screen and the next prompt follow, the old one is reachable")
def cd(c):
    sb, st, t = c.session()
    other = os.path.join(sb.root, "elsewhere")
    os.makedirs(other)
    c.say(t, "lemon one", expect="echo: lemon one")
    old = session_id(c, t)
    screen = answer(c, t, f"/cd {other}", expect="Now working in")
    status = [l for l in t.lines() if l.rstrip().endswith("? keys")]
    c.ok(status and "elsewhere" in status[-1], "/cd: the status line shows the new folder", screen)
    c.ok(status and "⎇" not in status[-1], "/cd: no git branch is shown for a folder outside git", screen)
    t.type("!pwd", settle=False)
    t.wait(re.compile(r"\$ pwd"), timeout=10)
    c.idle(t)
    c.ok(re.search(r"^\s*\S*elsewhere\s*$", t.text(), re.M), "/cd: shell commands run in the new folder", t.text())
    c.say(t, "lemon two", expect="lemon two")
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("lemon one" in sent, "/cd: the conversation came along", sent[-500:])
    screen = answer(c, t, "/cd /nonexistent/zzz")
    c.ok(re.search(r"not|no such|doesn't", screen, re.I) and t.status is None, "/cd to a missing folder says so", screen)
    screen = answer(c, t, "/cd")
    c.ok(t.status is None, "/cd with no argument answers", screen)
    c.quit(t)
    t = c.term(sb, st, args=["--resume", old])
    c.ok("echo: lemon one" in t.text() and "lemon two" not in t.text(), "the conversation left in the old folder resumes whole", t.text())
    c.quit(t)
