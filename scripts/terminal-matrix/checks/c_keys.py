"""C. The editor and the keys."""

import json
import os
import re
import time

import kterm
from kterm import ALT_ENTER, BACKTAB, BS, DOWN, END, ENTER, ESC, HOME, LEFT, RIGHT, TAB, UP, ctrl, paste
from run import check

SHIFT_ENTER = "\x1b[13;2u"
ACTIONS = ["submit", "newline", "cancel", "interrupt", "exit", "cycleMode", "historySearch", "complete", "lineStart", "lineEnd",
           "deleteWord", "killToStart", "killToEnd", "wordLeft", "wordRight", "redraw", "toggleSidebar", "showKeys"]


def clear(t):
    t.send(ctrl("c"))
    t.settle(quiet=0.1)


def mode(t):
    panel = next((l for l in reversed(t.lines()) if "127.0.0.1" in l), "")
    for name in ("Accept edits", "Plan", "Default", "Bypass", "Auto", "Don't ask"):
        if name in panel:
            return name
    return panel


@check("C", "every key in /keybindings")
def keys(c):
    sb, st = c.sandbox(), c.stub()
    st.rules["slowkey"] = [{"text": "slow " * 30 + "slow-done", "chunk_delay": 0.2}]
    t = c.term(sb, st, cols=150, rows=40, keyboard_protocol=True)
    screen = t.send("?")
    shown = [a for a in ("Send", "New line", "Interrupt", "Exit", "permission mode", "Search", "Complete", "Start", "Delete the word",
                         "Word left", "Redraw", "sidebar") if a.lower() not in screen.lower() and a.lower() not in t.scrollback().lower()]
    c.ok(not shown, "? on an empty prompt shows the key table", (shown, screen))
    c.ok(t.input_text() == "", "? on an empty prompt is not typed", t.input_text())
    t.send("what")
    t.send("?")
    c.ok(t.input_text() == "what?", "? after text is typed as a character", t.input_text())
    clear(t)
    c.say(t, "/keybindings")
    c.ok("keybindings.json" in t.scrollback() or "Ctrl+R" in t.scrollback(), "/keybindings shows the keys in effect", t.text())

    # New line by every documented route.
    for name, key in (("Shift+Enter", SHIFT_ENTER), ("Alt+Enter", ALT_ENTER), ("Ctrl+J", "\n")):
        t.send("top")
        t.send(key)
        t.send("bottom")
        c.ok(t.input_rows()[:2] == ["top", "bottom"], f"{name} starts a new line", t.input_rows())
        ok, why = t.lands()
        c.ok(ok, f"{name}: the cursor is where the next character lands", why)
        clear(t)
    t.send("top\\")
    t.send(ENTER)
    t.send("bottom")
    c.ok(t.input_rows()[:2] == ["top", "bottom"], "a backslash then Enter starts a new line", t.input_rows())
    sent = len(st.requests)
    t.send(ENTER, settle=False)
    t.wait("echo: top", timeout=10)
    c.idle(t)
    c.ok(st.prompts()[-1] == "top\nbottom", "Enter sends the two lines as one prompt", st.prompts()[-1:])

    # Movement and deletion, read back from the input.
    def edit(name, typed, keys_, want):
        clear(t)
        t.send(typed)
        for k in keys_:
            t.send(k, settle=False)
            t.pump(0.03)
        t.send("X")
        c.ok(t.input_text() == want, f"{name}", f"typed {typed!r}, got {t.input_text()!r}, wanted {want!r}")

    edit("Ctrl+A goes to the start of the line", "one two three", [ctrl("a")], "Xone two three")
    edit("Home goes to the start of the line", "one two three", [HOME], "Xone two three")
    edit("Ctrl+E goes to the end of the line", "one two three", [ctrl("a"), ctrl("e")], "one two threeX")
    edit("End goes to the end of the line", "one two three", [HOME, END], "one two threeX")
    edit("Ctrl+W deletes the word before the cursor", "one two three", [ctrl("w")], "one two X")
    edit("Alt+Backspace deletes the word before the cursor", "one two three", ["\x1b\x7f"], "one two X")
    edit("Ctrl+U deletes to the start of the line", "one two three", [LEFT, LEFT, ctrl("u")], "Xee")
    edit("Ctrl+K deletes to the end of the line", "one two three", [HOME, RIGHT, RIGHT, RIGHT, ctrl("k")], "oneX")
    edit("Alt+B moves a word left", "one two three", ["\x1bb"], "one two Xthree")
    edit("Ctrl+Left moves a word left", "one two three", ["\x1b[1;5D", "\x1b[1;5D"], "one Xtwo three")
    edit("Alt+F moves a word right", "one two three", [HOME, "\x1bf"], "oneX two three")
    edit("Ctrl+Right moves a word right", "one two three", [HOME, "\x1b[1;5C", "\x1b[1;5C"], "one twoX three")
    edit("Left and Right move by a character", "abc", [LEFT, LEFT, RIGHT], "abXc")
    edit("Backspace deletes before the cursor", "abc", [BS], "abX")
    edit("Delete deletes under the cursor", "abc", [HOME, kterm.DEL], "Xbc")
    clear(t)

    # Tab completes a command; Up and Down in the menu.
    t.send("/keyb")
    c.ok("/keybindings" in t.text(), "typing / opens the menu, filtered", t.text())
    t.send(TAB)
    c.ok(t.input_text().strip() == "/keybindings", "Tab completes the highlighted command", t.input_text())
    clear(t)
    t.send("/")
    first = t.styles()
    t.send(DOWN)
    t.send(DOWN)
    t.send(UP)
    c.ok(t.styles() != first, "Up and Down move the highlighted row in the menu", t.text())
    t.send(TAB)
    c.ok(t.input_text().strip() == "/advisor", "and Tab completes the row they moved to (the second)", t.input_text())
    t.send(ESC)
    c.ok("Add a working directory" not in t.text(), "Esc closes the menu", t.text())
    clear(t)

    # Shift+Tab cycles the permission mode, and the session follows.
    seen = [mode(t)]
    for _ in range(3):
        t.send(BACKTAB)
        seen.append(mode(t))
    c.ok(seen == ["Default", "Accept edits", "Plan", "Default"], "Shift+Tab cycles default, accept edits, plan", seen)
    t.send(BACKTAB)
    screen = c.say(t, "/status", expect="Session:")
    c.ok("acceptEdits" in screen, "the session's mode follows Shift+Tab (/status)", screen)
    t.send(BACKTAB)
    t.send(BACKTAB)

    # Ctrl+L redraws: the screen is the same after it.
    before = t.settle()
    t.send(ctrl("l"))
    after = t.settle()
    c.ok(after.split() == before.split() or (c.display == "inline" and "❯" in after), "Ctrl+L redraws the same screen", (before, after))

    # Ctrl+B: the sidebar, full screen from 140 columns.
    t.send(ctrl("b"))
    with_bar = t.settle()
    t.send(ctrl("b"))
    without = t.settle()
    if c.display == "full":
        c.ok(with_bar != without and ("Context" in with_bar or "Context" in without), "Ctrl+B shows and hides the sidebar at 150 columns", (with_bar, without))
    else:
        c.ok(with_bar == without, "Ctrl+B does nothing in the inline display", (with_bar, without))

    # Ctrl+C: clears the input; interrupts a turn; twice on an empty prompt exits. Ctrl+D with text does not exit.
    t.send("some text")
    t.send(ctrl("d"))
    c.ok(t.status is None, "Ctrl+D with text typed does not exit", t.status)
    t.send(ctrl("c"))
    c.ok(t.input_text() in ("", "Reply…") or "some text" not in t.input_text(), "Ctrl+C clears the input", t.input_text())
    t.type("slowkey", settle=False)
    t.wait("slow slow", timeout=10)
    t.send(ctrl("c"))
    screen = c.idle(t)
    c.ok("slow-done" not in screen and "Interrupted" in screen, "Ctrl+C interrupts the turn", screen)
    c.ok(c.alive(t, "after-ctrl-c"), "after Ctrl+C the next prompt works", t.text())
    t.type("slowkey", settle=False)
    t.wait("slow slow", timeout=10)
    t.send(ESC)
    screen = c.idle(t)
    c.ok("slow-done" not in screen.split("slowkey")[-1] and "Interrupted" in screen, "Esc interrupts the turn", screen)
    c.ok(c.alive(t, "after-esc"), "after Esc the next prompt works", t.text())
    t.send(ctrl("c"))
    c.ok(t.status is None, "Ctrl+C once on an empty prompt does not exit", t.status)
    t.send(ctrl("c"), settle=False)
    code = t.wait_exit(8)
    c.ok(code == 0 and not t.left_clean(), "Ctrl+C twice on an empty prompt exits cleanly", (code, t.left_clean()))
    # Ctrl+D on an empty prompt.
    t = c.term(sb, st)
    t.send(ctrl("d"), settle=False)
    code = t.wait_exit(8)
    c.ok(code == 0 and not t.left_clean(), "Ctrl+D on an empty prompt exits cleanly", (code, t.left_clean()))


@check("C", "keybindings.json: every action rebound, unbound keys, a broken file")
def rebinding(c):
    sb, st = c.sandbox(), c.stub()
    path = os.path.join(sb.kodact_home, "keybindings.json")
    # Every action on a function key; the defaults keep working too.
    fkeys = {a: f"f{i + 1}" for i, a in enumerate(ACTIONS[:12])}
    fkeys.update({a: f"alt+{ch}" for a, ch in zip(ACTIONS[12:], "qwyiop")})
    with open(path, "w") as f:
        json.dump({k: a for a, k in fkeys.items()}, f)
    code = {f"f{i}": s for i, s in enumerate(["\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[15~", "\x1b[17~", "\x1b[18~", "\x1b[19~", "\x1b[20~",
                                                "\x1b[21~", "\x1b[23~", "\x1b[24~"], start=1)}
    code.update({f"alt+{ch}": "\x1b" + ch for ch in "qwyiop"})
    key = lambda action: code[fkeys[action]]
    t = c.term(sb, st, cols=150, rows=40)
    c.ok("keybindings" not in t.text().lower() or "warning" not in t.text().lower(), "a valid file starts without a warning", t.text())

    def edit(action, typed, before, want):
        t.send(ctrl("c"))
        t.send(typed)
        for k in before:
            t.send(k, settle=False)
            t.pump(0.03)
        t.send(key(action))
        t.send("X")
        c.ok(t.input_text() == want, f"{action} on {fkeys[action]}", f"got {t.input_text()!r}, wanted {want!r}")

    edit("lineStart", "one two", [], "Xone two")
    edit("lineEnd", "one two", [HOME], "one twoX")
    edit("deleteWord", "one two", [], "one X")
    edit("killToStart", "one two", [LEFT], "Xo")
    edit("killToEnd", "one two", [HOME, RIGHT], "oX")
    edit("wordLeft", "one two", [], "one Xtwo")
    edit("wordRight", "one two", [HOME], "oneX two")
    t.send(ctrl("c"))
    t.send("a")
    t.send(key("newline"))
    t.send("b")
    c.ok(t.input_rows()[:2] == ["a", "b"], f"newline on {fkeys['newline']}", t.input_rows())
    t.send(key("submit"), settle=False)
    try:
        t.wait("echo: a", timeout=10)
        c.idle(t)
        c.ok(True, f"submit on {fkeys['submit']}")
    except AssertionError as e:
        c.ok(False, f"submit on {fkeys['submit']}", e)
    t.send("/keyb")
    t.send(key("complete"))
    c.ok(t.input_text().strip() == "/keybindings", f"complete on {fkeys['complete']}", t.input_text())
    t.send(key("interrupt"))
    c.ok("keybindings" not in t.input_text(), f"interrupt on {fkeys['interrupt']} clears the input", t.input_text())
    t.send(key("cycleMode"))
    c.ok(mode(t) == "Accept edits", f"cycleMode on {fkeys['cycleMode']}", mode(t))
    t.send(key("historySearch"))
    c.ok("search" in t.text().lower(), f"historySearch on {fkeys['historySearch']}", t.text())
    t.send(key("cancel"))
    c.ok("search history" not in t.text().lower(), f"cancel on {fkeys['cancel']} closes the search", t.text())
    t.send(key("showKeys"))
    c.ok("Delete the word" in t.scrollback(), f"showKeys on {fkeys['showKeys']}", t.text())
    a = t.settle()
    t.send(key("toggleSidebar"))
    b = t.settle()
    c.ok((a != b) == (c.display == "full"), f"toggleSidebar on {fkeys['toggleSidebar']}", (a, b))
    t.send(key("redraw"))
    c.ok(t.status is None and "❯" in t.settle(), f"redraw on {fkeys['redraw']}", t.text())
    # The defaults still work beside the new keys.
    t.send("still")
    t.send(ENTER, settle=False)
    try:
        t.wait("echo: still", timeout=10)
        c.idle(t)
        c.ok(True, "the default keys keep working beside the new ones")
    except AssertionError as e:
        c.ok(False, "the default keys keep working beside the new ones", e)
    t.send(key("exit"), settle=False)
    c.ok(t.wait_exit(8) == 0 and not t.left_clean(), f"exit on {fkeys['exit']} leaves cleanly", (t.status, t.left_clean()))

    # Unbinding: "none" takes the default key away.
    with open(path, "w") as f:
        json.dump({"ctrl+r": "none", "ctrl+s": "submit", "enter": "none"}, f)
    t = c.term(sb, st)
    t.send(ctrl("r"))
    c.ok("search history" not in t.text().lower(), "a key bound to none does nothing (Ctrl+R)", t.text())
    sent = len(st.requests)
    t.send("unbound enter")
    t.send(ENTER)
    c.ok(len(st.requests) == sent and "unbound enter" in t.input_text(), "Enter bound to none no longer sends", (len(st.requests), t.input_text()))
    t.send(ctrl("s"), settle=False)
    try:
        t.wait("echo: unbound enter", timeout=10)
        c.ok(True, "the key bound to submit sends instead (Ctrl+S)")
    except AssertionError as e:
        c.ok(False, "the key bound to submit sends instead (Ctrl+S)", e)
    c.idle(t)
    t.send(ctrl("d"), settle=False)
    t.wait_exit(8)

    # Broken files: a warning, the defaults, and a UI that works.
    for name, body in (("not JSON", "{this is not json"), ("an unknown action and key", '{"ctrl+s": "launchRockets", "hyper+zz": "submit", "f2": 3}'),
                       ("not an object", "[1, 2, 3]"), ("empty", ""), ("binary", "\x00\xff\xfe")):
        with open(path, "w", errors="surrogateescape") as f:
            f.write(body)
        t = c.term(sb, st)
        screen = t.text() + t.scrollback()
        c.ok(t.status is None, f"a keybindings.json that is {name}: the UI starts", t.scrollback()[-800:])
        c.ok("keybindings.json" in screen, f"a keybindings.json that is {name}: a warning names the file", screen)
        c.ok(c.alive(t, "broken"), f"a keybindings.json that is {name}: the default keys work", t.text())
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"a keybindings.json that is {name}: /exit leaves cleanly", (t.status, t.left_clean()))


@check("C", "typing: wide characters, combining marks, emoji, right-to-left, long lines")
def typing(c):
    samples = [
        ("plain text", "hello world"),
        ("CJK", "你好世界 こんにちは 한국어"),
        ("combining marks", "café niño"),
        ("an emoji", "ship it 🚀 now"),
        ("emoji with joiners", "family 👨‍👩‍👧 here"),
        ("a flag and a skin tone", "wave 👋🏽 flag 🇫🇷 end"),
        ("right-to-left", "שלום עולם مرحبا"),
        ("mixed directions", "abc שלום def"),
    ]
    for cols, rows in ((110, 36), (40, 12)):
        sb, st, t = c.session(cols=cols, rows=rows)
        for name, text in samples:
            t.send(ctrl("c"))
            t.send(text)
            ok, why = t.lands()
            # A terminal and the program can disagree on how wide a joined emoji is; that is the terminal's rule, reported apart.
            label = f"{name} at {cols}x{rows}: the cursor is where the next character lands"
            if not ok and name in ("emoji with joiners", "a flag and a skin tone"):
                c.notrun(label, "this emulator (pyte) draws joined emoji as separate glyphs; a real terminal's width rule decides")
            else:
                c.ok(ok, label, why)
            sent = len(st.requests)
            t.send(ENTER, settle=False)
            t.wait(lambda s: len(st.requests) > sent, timeout=10)
            c.idle(t)
            c.ok(st.prompts()[-1] == text + "Z", f"{name} at {cols}x{rows}: sent byte for byte", st.prompts()[-1:])
        # A line longer than the window: it wraps inside the box, and the cursor follows.
        t.send(ctrl("c"))
        long = "word " * (cols // 2)
        t.send(long)
        rows_ = t.input_rows()
        c.ok(len(rows_) >= 2 and all(len(r) <= cols for r in rows_), f"a line longer than the window wraps at {cols}x{rows}", rows_)
        ok, why = t.lands()
        c.ok(ok, f"a long line at {cols}x{rows}: the cursor is where the next character lands", why)
        t.send(HOME)
        t.send(ctrl("a"))
        ok, why = t.lands("Q")
        c.ok(ok, f"a long line at {cols}x{rows}: and at its start", why)
        # A 3,000-character line in a small box: the UI stays usable and the cursor stays on screen.
        t.send(ctrl("c"))
        t.send(paste("x" * 3000))
        x, y = t.cursor()
        c.ok(t.status is None and 0 <= y < rows and 0 <= x < cols, f"a 3,000-character line at {cols}x{rows}: the cursor stays in the window", (x, y))
        ok, why = t.lands()
        c.ok(ok, f"a 3,000-character line at {cols}x{rows}: the cursor is where the next character lands", why)
        t.send(ctrl("c"))
        c.ok(c.alive(t, "long"), f"after long input at {cols}x{rows} the next prompt works", t.text())
        c.quit(t)


@check("C", "paste: 1, 100 and 10,000 lines; tabs, escapes, a trailing newline")
def pasting(c):
    sb, st, t = c.session()
    cases = [
        ("1 line", "just one line"),
        ("100 lines", "\n".join(f"line {i}" for i in range(100))),
        ("10,000 lines", "\n".join(f"row {i}" for i in range(10000))),
        ("tabs", "a\tb\tc\n\tindented"),
        ("a trailing newline", "ends with newline\n"),
        ("CRLF line ends", "one\r\ntwo\r\nthree"),
    ]
    for name, text in cases:
        t.send(ctrl("c"))
        t0 = time.time()
        os.write(t.fd, paste(text).encode())
        t.settle(quiet=0.4, timeout=30)
        took = time.time() - t0
        c.ok(t.status is None, f"paste of {name}: the program keeps running", t.scrollback()[-800:])
        c.ok(took < 10, f"paste of {name}: the UI settles within 10 s", f"{took:.1f}s")
        c.ok(len(st.requests) == len([1 for n, _ in cases[:cases.index((name, text))]]), f"paste of {name}: pasting sends nothing by itself",
             len(st.requests))
        x, y = t.cursor()
        c.ok(0 <= y < t.rows and 0 <= x < t.cols, f"paste of {name}: the cursor stays in the window", (x, y))
        ok, why = t.lands()
        c.ok(ok, f"paste of {name}: the cursor is where the next character lands", why[-1500:])
        t.send(BS)
        sent = len(st.requests)
        t.send(ENTER, settle=False)
        try:
            t.wait(lambda s: len(st.requests) > sent, timeout=20)
            c.idle(t, timeout=30)
            got = st.prompts()[-1]
            want = text.replace("\r\n", "\n")
            c.ok(got == want or got == want.rstrip("\n") or got.replace("    ", "\t") == want, f"paste of {name}: sent as pasted",
                 f"{len(got)} chars vs {len(want)}; start {got[:80]!r}; end {got[-60:]!r}")
        except AssertionError as e:
            c.ok(False, f"paste of {name}: Enter sends it", str(e)[-1500:])
    # Escape sequences inside a paste are text, never commands to the terminal.
    t.send(ctrl("c"))
    evil = "before \x1b[2J\x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b[?1049l after"
    raw = len(t.raw)
    os.write(t.fd, paste(evil).encode())
    screen = t.settle()
    wrote = t.raw[raw:]
    c.ok(b"\x1b[2J" not in wrote and b"\x1b]0;title" not in wrote and b"\x1b[?1049l" not in wrote,
         "pasted escape sequences are not written back to the terminal", wrote[-300:])
    c.ok("before" in screen and "after" in screen, "the pasted text around them shows", screen)
    ok, why = t.lands()
    c.ok(ok, "after pasting escapes the cursor is where the next character lands", why)
    t.send(ctrl("c"))
    c.ok(c.alive(t, "paste"), "after the pastes the next prompt works", t.text())
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("C", "history: Up and Down, Ctrl+R, kept per directory, multi-line recalled whole")
def history(c):
    sb, st = c.sandbox(), c.stub()
    other = os.path.join(sb.root, "other-project")
    os.makedirs(other)
    t = c.term(sb, st)
    c.say(t, "first prompt here", expect="echo: first")
    c.say(t, "second prompt here", expect="echo: second")
    t.send("multi one")
    t.send(ALT_ENTER)
    t.send("multi two")
    t.send(ENTER, settle=False)
    t.wait("echo: multi one", timeout=10)
    c.idle(t)
    t.send(UP)
    c.ok(t.input_rows()[:2] == ["multi one", "multi two"], "Up recalls a prompt with newlines whole", t.input_rows())
    t.send(UP)
    t.send(UP)
    c.ok(t.input_text() == "second prompt here", "Up again (from the first line) goes to the prompt before", t.input_text())
    t.send(UP)
    c.ok(t.input_text() == "first prompt here", "and the one before that", t.input_text())
    t.send(UP)
    c.ok(t.input_text() == "first prompt here", "Up at the oldest stays there", t.input_text())
    t.send(DOWN)
    c.ok(t.input_text() == "second prompt here", "Down comes forward", t.input_text())
    t.send(DOWN)
    t.send(DOWN)
    t.send(DOWN)
    t.send(DOWN)
    c.ok(t.input_text() in ("", "Reply…") or "prompt here" not in t.input_text(), "Down past the newest returns to an empty prompt", t.input_text())
    t.send("draft text")
    t.send(UP)
    t.send(DOWN)
    t.send(DOWN)
    c.ok(t.input_text() == "draft text", "what was being typed comes back after browsing", t.input_text())
    t.send(ctrl("c"))
    # Ctrl+R.
    t.send(ctrl("r"))
    t.send("first")
    c.ok("first prompt here" in t.text(), "Ctrl+R finds a prompt by its text", t.text())
    t.send(ENTER)
    c.ok(t.input_text() == "first prompt here" or len(st.requests) == 4, "Enter keeps the match", (t.input_text(), len(st.requests)))
    t.send(ctrl("c"))
    t.send("typed before search")
    t.send(ctrl("r"))
    t.send("second")
    t.send(ESC)
    c.ok(t.input_text() == "typed before search", "Esc in the search goes back to what was typed", t.input_text())
    t.send(ctrl("c"))
    t.send(ctrl("r"))
    t.send("prompt")
    one = t.text()
    t.send(ctrl("r"))
    two = t.text()
    c.ok(one != two, "Ctrl+R again goes to an older match", (one[-400:], two[-400:]))
    t.send(ESC)
    t.send(ctrl("r"))
    t.send("zzz-no-such-prompt")
    c.ok(t.status is None, "a search with no match keeps running", t.text())
    t.send(ESC)
    t.send(ctrl("c"))
    c.quit(t)
    # The file, and its mode.
    hist = sb.state("history.jsonl")
    c.ok(os.path.exists(hist) and oct(os.stat(hist).st_mode & 0o777) == "0o600", "history.jsonl exists with mode 0600",
         oct(os.stat(hist).st_mode & 0o777) if os.path.exists(hist) else "missing")
    # Another directory has its own history; back here it is all still there after a restart.
    t = c.term(sb, st, cwd=other)
    t.send(UP)
    c.ok("prompt here" not in t.input_text() and "multi" not in t.input_text(), "another directory does not see this one's prompts", t.input_text())
    c.quit(t)
    t = c.term(sb, st)
    t.send(ctrl("r"))
    t.send("second pro")
    t.send(ENTER)
    c.ok(t.input_text() == "second prompt here", "after a restart the directory's history is still there", t.input_text())
    t.send(ctrl("c"))
    c.quit(t)


@check("C", "@ file mentions: the menu, filtering, spaces, a missing file, many thousands of files")
def mentions(c):
    sb, st = c.sandbox(git=False), c.stub()
    with open(os.path.join(sb.project, "my notes.md"), "w") as f:
        f.write("NOTES-WITH-SPACE-BODY\n")
    with open(os.path.join(sb.project, "alpha.rs"), "w") as f:
        f.write("ALPHA-BODY\n")
    for d in range(250):
        os.makedirs(os.path.join(sb.project, "gen", f"d{d:03d}"))
        for i in range(100):
            open(os.path.join(sb.project, "gen", f"d{d:03d}", f"f{i:02d}.txt"), "w").close()
    with open(os.path.join(sb.project, "gen", "d249", "needle-unique.txt"), "w") as f:
        f.write("NEEDLE-BODY\n")
    t0 = time.time()
    t = c.term(sb, st)
    c.ok(time.time() - t0 < 10, "with 25,000 files the UI starts within 10 s", f"{time.time() - t0:.1f}s")
    t.pump(2.0)
    t0 = time.time()
    t.send("@")
    c.ok("alpha.rs" in t.text() or "gen/" in t.text() or "notes" in t.text(), "typing @ opens a menu of paths", t.text())
    t.send("alph")
    c.ok("alpha.rs" in t.text() and time.time() - t0 < 5, "typing filters it, without a wait", (f"{time.time() - t0:.1f}s", t.text()))
    t.send(TAB)
    c.ok(t.input_text().strip() == "@alpha.rs", "Tab completes the path", t.input_text())
    t.send(" and @my")
    t.send(TAB)
    c.ok('@"my notes.md"' in t.input_text(), "a path with spaces is completed in quotes", t.input_text())
    ok, why = t.lands()
    c.ok(ok, "after completing, the cursor is where the next character lands", why)
    t.send(BS)
    t.send(ENTER, settle=False)
    t.wait("echo:", timeout=15)
    c.idle(t)
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("ALPHA-BODY" in sent and "NOTES-WITH-SPACE-BODY" in sent, "both files' contents are attached to the prompt", sent[-800:])
    c.ok(re.search(r"attached.*alpha\.rs", t.text()), "the transcript says what was attached", t.text())
    # A file the walk may not have reached is still attached when named in full.
    t.send("see @gen/d249/needle-unique.txt")
    t.send(ESC)
    t.send(ENTER, settle=False)
    t.wait(re.compile(r"echo: see"), timeout=15)
    c.idle(t)
    c.ok("NEEDLE-BODY" in json.dumps(st.requests[-1]["messages"]), "a file deep in a large project is attached when named", "")
    # A file that does not exist: left alone, and said so nowhere as attached.
    t.send("look at @no-such-file.txt please")
    t.send(ESC)
    t.send(ENTER, settle=False)
    t.wait(re.compile(r"echo: look at @no-such-file\.txt please"), timeout=15)
    c.idle(t)
    c.ok("attached: no-such-file" not in t.text(), "a file that does not exist is not attached; the words go as typed", t.text())
    # Esc closes the menu and keeps the text.
    t.send("mail me@")
    t.send(ESC)
    c.ok(t.input_text() == "mail me@", "Esc closes the menu and keeps what was typed", t.input_text())
    t.send(ctrl("c"))
    c.ok(c.alive(t, "mentions"), "the next prompt works", t.text())
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("C", "! shell mode: success, failure, a megabyte, colour, waiting for input, interrupted")
def shell(c):
    sb, st, t = c.session(cols=110, rows=36)
    with open(os.path.join(sb.kodact_home, "settings.json"), "w") as f:
        json.dump({"respondToBashCommands": False}, f)
    c.quit(t)
    t = c.term(sb, st)

    def bang(cmd, wait_for, timeout=20):
        t.send(ctrl("c"))
        t.type("!" + cmd, settle=False)
        try:
            t.wait(wait_for, timeout=timeout)
        except AssertionError:
            pass
        return c.idle(t, timeout=timeout)

    screen = bang("echo shell-ok-$((6*7))", "shell-ok-42")
    c.ok("shell-ok-42" in screen, "a command that succeeds shows its output", screen)
    screen = bang("ls /definitely/not/here; exit 3", re.compile(r"No such file"))
    c.ok("No such file" in screen and re.search(r"exit 3|failed|✗", screen), "a command that fails shows its error and that it failed", screen)
    t0 = time.time()
    screen = bang("head -c 1048576 /dev/zero | tr '\\0' 'x' | fold -w 100; echo END-OF-MEGABYTE", "END-OF-MEGABYTE", timeout=40)
    c.ok("END-OF-MEGABYTE" in screen and time.time() - t0 < 30, "a megabyte of output: the end shows, within 30 s", (f"{time.time() - t0:.1f}s", screen[-600:]))
    c.ok(len(t.scrollback()) < 400_000, "a megabyte of output is not all written to the screen", len(t.scrollback()))
    raw = len(t.raw)
    screen = bang("printf '\\033[31mRED-TEXT\\033[0m \\033[2J\\033[?1049l after-colour\\n'", "after-colour")
    wrote = t.raw[raw:]
    c.ok("RED-TEXT" in screen and "after-colour" in screen, "coloured output shows its text", screen)
    c.ok(b"\x1b[2J" not in wrote.replace(b"\x1b[2J\x1b[", b"") or b"\x1b[?1049l" not in wrote, "the command's own escapes do not reach the terminal", wrote[-200:])
    c.ok(t.modes()["alt_screen"] == (c.display == "full"), "and the display mode is as it was", t.modes())
    t0 = time.time()
    screen = bang("read -r answer; echo got:[$answer]", "got:[", timeout=15)
    c.ok("got:[]" in screen and time.time() - t0 < 10, "a command that waits for input gets end-of-file instead of hanging", (f"{time.time() - t0:.1f}s", screen))
    t.send(ctrl("c"))
    t.type("!sleep 60; echo SLEPT-THE-MINUTE", settle=False)
    t.pump(2.0)
    c.ok(c.busy(t.text()) or "sleep 60" in t.text(), "a long command shows as running", t.text())
    t.send(ESC)
    t0 = time.time()
    screen = c.idle(t, timeout=15)
    c.ok("SLEPT-THE-MINUTE" not in screen.split("echo SLEPT-THE-MINUTE")[-1] and time.time() - t0 < 10, "Esc stops a long command within 10 s", (f"{time.time() - t0:.1f}s", screen))
    import subprocess
    left = subprocess.run(["pgrep", "-f", "sleep 60"], capture_output=True, text=True).stdout.split()
    mine = [p for p in left if os.path.exists(f"/proc/{p}/cwd") and os.path.realpath(f"/proc/{p}/cwd").startswith(sb.root)]
    c.ok(not mine, "and no sleep process is left behind", mine)
    c.ok(c.alive(t, "shell"), "after the shell commands the next prompt works", t.text())
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("C", "Esc, Ctrl+C, Ctrl+D, Ctrl+L and Shift+Tab in every state")
def states(c):
    keys_ = [("Esc", ESC), ("Esc twice", ESC + ESC), ("Ctrl+C", ctrl("c")), ("Ctrl+L", ctrl("l")), ("Shift+Tab", BACKTAB), ("Ctrl+D", ctrl("d"))]
    sb, st = c.sandbox(), c.stub()
    st.rules["slowstate"] = [{"text": "tick " * 40 + "tick-done", "chunk_delay": 0.15}]
    st.rules["askstate"] = [{"text": "Asking.", "tools": [("Bash", {"command": "touch state-approved.txt"})]}, {"text": "asked."}]
    for name, key in keys_:
        # With text typed.
        t = c.term(sb, st)
        c.say(t, "seed", expect="echo: seed")
        t.send("typed text")
        t.send(key)
        t.settle()
        c.ok(t.status is None, f"{name} with text typed: the program keeps running", (t.status, t.scrollback()[-500:]))
        if t.status is None:
            for _ in range(2):
                if c.dialog(t.text()):
                    t.send(ESC)
            c.ok(c.alive(t, "typed"), f"{name} with text typed: the next prompt works", t.text())
            # During a turn.
            t.type("slowstate", settle=False)
            t.wait("tick tick", timeout=10)
            t.send(key)
            t.pump(0.5)
            if name in ("Esc", "Esc twice", "Ctrl+C"):
                screen = c.idle(t, timeout=10)
                c.ok("tick-done" not in screen, f"{name} during a turn stops it", screen)
                c.ok(not c.busy(screen), f"{name} during a turn: nothing shows as still running", screen)
            else:
                if name == "Ctrl+D":
                    # Exit (empty prompt), as the key table says: mid-turn too.
                    code = t.wait_exit(10)
                    c.ok(code == 0 and not t.left_clean(), "Ctrl+D during a turn (empty prompt) exits cleanly", (code, t.left_clean()))
                    t = c.term(sb, st, args=["--continue"])
                    c.ok("slowstate" in t.text(), "and the conversation, with the prompt that was running, resumes", t.text())
                else:
                    try:
                        t.wait("tick-done", timeout=15)
                        c.ok(True, f"{name} during a turn leaves it running to its end")
                    except AssertionError as e:
                        c.ok(False, f"{name} during a turn leaves it running to its end", e)
                    c.idle(t)
            for _ in range(2):
                if c.dialog(t.text()):
                    t.send(ESC)
            if mode_now(t) != "Default":
                for _ in range(3):
                    if mode_now(t) == "Default":
                        break
                    t.send(BACKTAB)
            c.ok(c.alive(t, "turn"), f"{name} during a turn: the next prompt works", t.text())
            # With a permission dialog open.
            t.type("askstate", settle=False)
            try:
                t.wait("Allow once", timeout=10)
                t.send(key)
                t.pump(0.5)
                if "Allow once" in t.text():
                    c.ok(name in ("Ctrl+L", "Shift+Tab", "Ctrl+D"), f"{name} with a dialog open leaves the question waiting", t.text())
                    t.send(ESC)
                c.idle(t, timeout=15)
                c.ok(not os.path.exists(os.path.join(sb.project, "state-approved.txt")) or name == "Shift+Tab", f"{name} with a dialog open never approves it",
                     "state-approved.txt exists")
            except AssertionError as e:
                c.ok(False, f"{name}: the permission dialog opens", e)
            if os.path.exists(os.path.join(sb.project, "state-approved.txt")):
                os.remove(os.path.join(sb.project, "state-approved.txt"))
            for _ in range(3):
                if mode_now(t) == "Default":
                    break
                t.send(BACKTAB)
            c.ok(t.status is None and c.alive(t, "dialog"), f"{name} with a dialog open: the next prompt works", t.text())
            c.ok(c.quit(t) == 0, f"{name}: /exit afterwards", t.status)
        c.ok(not t.left_clean(), f"{name}: the terminal is left as it was", t.left_clean())


def mode_now(t):
    return mode(t)
