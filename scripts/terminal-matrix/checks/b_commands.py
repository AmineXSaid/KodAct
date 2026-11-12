"""B. Every slash command, every form. The list comes from the registry (kterm.builtins)."""

import json
import os
import re

import kterm
from kterm import DOWN, ENTER, ESC, PGDN, PGUP, TAB, UP, ctrl
from run import check

BUILTINS = kterm.builtins()
SLOW = "slowturn"


def close_dialogs(c, t):
    for _ in range(4):
        if not c.dialog(t.text()):
            return
        t.send(ESC)


def settle_turn(c, t, limit=12):
    """Let what the command started end; stop it with Esc if it runs on. Returns whether it had to be stopped."""
    try:
        c.idle(t, timeout=limit)
        return False
    except AssertionError:
        t.send(ESC)
        c.idle(t, timeout=10)
        return True


def restore(c, t):
    """Undo what a form may have left on: another model, another permission mode."""
    close_dialogs(c, t)
    t.send(ctrl("u"))
    panel = lambda: next((l for l in reversed(t.lines()) if "127.0.0.1" in l), "")
    for _ in range(6):
        if re.search(r"\bDefault\b", panel()):
            break
        t.send(kterm.BACKTAB)
    if "stub-model" not in panel() or "✗" in panel():
        t.type("/model stub-model", settle=False)
        t.settle()
        settle_turn(c, t)


def run_form(c, t, st, text, label):
    """Type one form of a command; check the UI answered and still works. Returns the screen after it."""
    close_dialogs(c, t)
    t.send(ctrl("u"))
    before, sent = t.text(), len(st.requests)
    t.type(text, settle=False)
    t.settle()
    settle_turn(c, t)
    screen = t.text()
    if t.status is not None:
        c.ok(False, f"{label}: the program keeps running", f"exit {t.status}\n{t.scrollback()[-2500:]}")
        return screen
    c.ok(screen != before, f"{label}: something answers on screen", screen)
    c.ok("panicked" not in t.scrollback(), f"{label}: no panic", t.scrollback()[-1500:])
    return screen


def one_command(name, aliases, args):
    def body(c):
        sb, st, t = c.session()
        st.rules[SLOW] = [{"text": "slow answer " + "word " * 12 + "end-of-slow", "chunk_delay": 0.25}]
        st.rules["needs-approval"] = [{"text": "Running.", "tools": [("Bash", {"command": "touch approved.txt"})]}, {"text": "ran."}]
        for n in [name, *aliases]:
            # On the empty front page.
            run_form(c, t, st, f"/{n}", f"/{n} on the front page")
            restore(c, t)
            c.ok(c.alive(t, f"{n}-front"), f"/{n} on the front page: the next prompt works", t.text())
            # After a turn: no argument, a wrong one, too many, spaces around, upper case.
            forms = [("no argument", f"/{n}"), ("a wrong argument", f"/{n} zzz-wrong-arg"),
                     ("too many arguments", f"/{n} one two three four five six seven"),
                     ("spaces around", f"  /{n}  "), ("upper case", f"/{n.upper()}")]
            for i, (what, text) in enumerate(forms):
                sent = len(st.requests)
                screen = run_form(c, t, st, text, f"/{n} with {what}")
                if what in ("spaces around", "upper case"):
                    leaked = [p for p in st.prompts()[sent:] if p.strip().lower() == f"/{n}"]
                    c.ok(not leaked, f"/{n} with {what}: it is not sent to the model as words", leaked)
                restore(c, t)
                c.ok(c.alive(t, f"{n}-{i}"), f"/{n} with {what}: the next prompt works", t.text())
            # From the menu: type it, Tab completes, Enter runs.
            close_dialogs(c, t)
            t.send(ctrl("u"))
            t.type("/" + n, enter=False)
            menu = t.text()
            if n == name:
                c.ok(re.search(rf"/{re.escape(n)}\b.*\S", menu), f"/{n}: the menu lists it with its description", menu)
            t.send(TAB)
            c.ok(f"/{name}" in t.text() or f"/{n}" in t.text(), f"/{n}: Tab completes it in the input", t.text())
            t.send(ENTER)
            if f"❯ /{name} " in t.text() or f"❯ /{n} " in t.text():
                t.send(ENTER)  # a command that takes arguments is completed first, run second
            settle_turn(c, t)
            c.ok(t.status is None, f"/{n} from the menu: the program keeps running", t.scrollback()[-1500:])
            restore(c, t)
            c.ok(c.alive(t, f"{n}-menu"), f"/{n} from the menu: the next prompt works", t.text())
            # While a turn runs: answered at once or queued, and the turn's answer still arrives.
            close_dialogs(c, t)
            t.type(SLOW, settle=False)
            t.wait("slow answer", timeout=10)
            t.type(f"/{n}", settle=False)
            t.pump(0.4)
            mid = t.text()
            try:
                t.wait("end-of-slow", timeout=15)
                c.ok(True, f"/{n} during a turn: the turn's answer still arrives")
            except AssertionError as e:
                c.ok(False, f"/{n} during a turn: the turn's answer still arrives", e)
            settle_turn(c, t)
            c.ok(t.status is None and "panicked" not in t.scrollback(), f"/{n} during a turn: the program keeps running",
                 t.scrollback()[-1500:])
            restore(c, t)
            c.ok(c.alive(t, f"{n}-mid"), f"/{n} during a turn: the next prompt works", t.text())
            # While a picker is open: the keys go to the picker; Esc leaves it as it was.
            close_dialogs(c, t)
            t.type("/model", settle=False)
            try:
                t.wait("Select a model", timeout=8)
                t.type(f"/{n}", settle=False)
                t.settle()
                t.send(ESC)
                close_dialogs(c, t)
                c.ok(t.status is None, f"/{n} typed into an open picker: the program keeps running", t.scrollback()[-1500:])
                c.ok(c.alive(t, f"{n}-picker"), f"/{n} typed into an open picker, then Esc: the next prompt works", t.text())
            except AssertionError as e:
                c.ok(False, f"/{n}: the /model picker opens to type into", e)
            # While a permission question is open: it keeps waiting for its own answer.
            restore(c, t)
            t.type("needs-approval", settle=False)
            try:
                t.wait("Allow once", timeout=10)
                t.send(ESC)
                c.idle(t)
                c.ok(not os.path.exists(os.path.join(sb.project, "approved.txt")), f"/{n}: Esc on a permission question denies it",
                     "approved.txt exists")
            except AssertionError as e:
                c.ok(False, f"/{n}: a permission question opens and Esc closes it", e)
            c.ok(c.alive(t, f"{n}-perm"), f"/{n}: after a denied question the next prompt works", t.text())
        c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly afterwards", (t.status, t.left_clean()))
    return body


for _name, _aliases, _args, _tui, _imm in BUILTINS:
    if _name == "exit":
        continue
    check("B", f"/{_name}: every form")(one_command(_name, _aliases, _args))


@check("B", "/exit and /quit leave the terminal as they found it")
def exits(c):
    for cmd in ("/exit", "/quit", "  /exit  ", "/exit now please"):
        sb, st, t = c.session()
        c.say(t, "remember this line", expect="echo: remember this line")
        t.type(cmd, settle=False)
        code = t.wait_exit(10)
        c.ok(code == 0, f"{cmd!r}: exit status 0", (code, t.text()))
        modes = t.modes()
        for mode in ("alt_screen", "mouse", "cursor_hidden", "bracketed_paste", "keyboard_protocol"):
            c.ok(not modes[mode], f"{cmd!r}: {mode.replace('_', ' ')} is off", modes)
        c.ok(t.tty_restored(), f"{cmd!r}: raw mode is off", "")
        left = t.after_alt_screen() if c.display == "full" else t.scrollback()
        c.ok("remember this line" in left and "echo: remember this line" in left, f"{cmd!r}: the transcript is in the scrollback", left[-1500:])
        c.ok(re.search(r"Resume this conversation with: kodact --resume [0-9a-f-]{36}", left), f"{cmd!r}: the resume hint is printed", left[-400:])
    # During a turn.
    sb, st, t = c.session()
    st.rules[SLOW] = [{"text": "slow " * 40, "chunk_delay": 0.2}]
    t.type(SLOW, settle=False)
    t.wait("slow slow", timeout=10)
    t.send(ctrl("c"), settle=False)
    t.pump(0.3)
    t.send(ctrl("c"), settle=False)
    t.pump(0.2)
    t.send(ctrl("c"), settle=False)
    code = t.wait_exit(10)
    c.ok(code == 0 and not t.left_clean(), "Ctrl+C during a turn, then twice more: leaves cleanly", (code, t.left_clean(), t.text()))


@check("B", "an unknown command, a lone slash, and names that shadow a built-in")
def unknown(c):
    sb, st = c.sandbox(), c.stub()
    os.makedirs(os.path.join(sb.project, ".kodact", "commands"))
    os.makedirs(os.path.join(sb.project, ".kodact", "skills", "status"))
    with open(os.path.join(sb.project, ".kodact", "commands", "clear.md"), "w") as f:
        f.write("---\ndescription: a custom clear\n---\nCUSTOM-CLEAR-BODY\n")
    with open(os.path.join(sb.project, ".kodact", "commands", "greet.md"), "w") as f:
        f.write("---\ndescription: say hello\n---\nCUSTOM-GREET-BODY $ARGUMENTS\n")
    with open(os.path.join(sb.project, ".kodact", "skills", "status", "SKILL.md"), "w") as f:
        f.write("---\nname: status\ndescription: a skill named status\n---\nSKILL-STATUS-BODY\n")
    t = c.term(sb, st)
    c.say(t, "first turn", expect="echo: first turn")
    sent = len(st.requests)
    screen = c.say(t, "/nosuchcommand")
    c.ok("Unknown command" in screen and "/nosuchcommand" in screen, "an unknown command says so", screen)
    c.ok(len(st.requests) == sent, "and is not sent to the model", st.prompts()[sent:])
    screen = c.say(t, "/")
    c.ok(t.status is None and len(st.requests) == sent, "a lone / is not sent to the model", (st.prompts()[sent:], screen))
    c.ok(c.alive(t, "slash"), "after a lone / the next prompt works", t.text())
    sent = len(st.requests)
    c.say(t, "/greet you", expect="echo")
    c.ok(any("CUSTOM-GREET-BODY you" in p for p in st.prompts()[sent:]), "a custom command sends its body with the arguments", st.prompts()[sent:])
    # A custom command file named like a built-in does not replace it; a skill of yours does
    # (CLI.md, "Bundled skills": as in the reference CLI).
    sent = len(st.requests)
    screen = c.say(t, "/clear")
    c.ok("Conversation cleared" in screen and not any("CUSTOM-CLEAR-BODY" in p for p in st.prompts()[sent:]),
         "a custom command file named clear does not replace /clear", (screen, st.prompts()[sent:]))
    c.say(t, "after clear", expect="echo: after clear")
    sent = len(st.requests)
    c.say(t, "/status", expect="SKILL-STATUS-BODY")
    c.ok(any("SKILL-STATUS-BODY" in p for p in st.prompts()[sent:]), "a skill of yours named status replaces /status, as documented",
         st.prompts()[sent:])
    c.quit(t)
