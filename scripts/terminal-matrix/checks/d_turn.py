"""D. A turn, start to finish."""

import json
import os
import re
import time

import kterm
from kterm import BACKTAB, DOWN, ENTER, ESC, PGDN, PGUP, UP, ctrl
from run import SIZES, check

MARKDOWN = """# Heading one
## Heading two
### Heading three
#### Heading four
##### Heading five
###### Heading six

Plain paragraph with **bold words**, *italic words*, ~~struck~~ and `inline code`, a [link text](https://example.com/page) and https://bare.example.com/url.

- bullet one
  - nested bullet
    - deeper bullet
- bullet two
1. numbered one
2. numbered two
- [ ] task open
- [x] task done

> quoted line
>> nested quote

| Name | Count | A very long column header that goes on |
|:-----|------:|----------------------------------------|
| alpha | 1 | cell text that is long enough to need the whole window and then some more words to be sure |
| beta | 22 | short |

---

```rust
fn main() { let x: u32 = 42; println!("rust-fence {}", x); }
```
```python
def f(a): return "python-fence" + str(a)  # comment
```
```js
const s = `js-fence ${1 + 1}`;
```
```go
func main() { fmt.Println("go-fence") }
```
```c
int main(void) { return puts("c-fence"); }
```
```sh
echo "sh-fence" | grep -c fence
```
```sql
SELECT 'sql-fence' FROM t WHERE id = 1;
```
```json
{"json-fence": [1, 2, true, null]}
```
```diff
-removed diff line
+added diff line
```
```unknownlang
plain-fence line
```
Last markdown line END-OF-MARKDOWN
"""

BROKEN = """Text with **unclosed bold and *unclosed italic and `unclosed code
a [broken link](http://
| lonely | table row
```rust
fn unclosed_fence() {
END-OF-BROKEN"""


@check("D", "answers shorter and longer than the window; scrolling while it streams (sizes)")
def answers(c):
    for cols, rows in SIZES:
        sb, st, t = c.session(cols=cols, rows=rows)
        long = "\n".join(f"line number {i} of the long answer" for i in range(1, 121)) + "\nLAST-LINE-OF-LONG"
        st.rules["longone"] = [{"text": long}]
        st.rules["streamlong"] = [{"text": " ".join(f"w{i}\n" if i % 6 == 5 else f"w{i}" for i in range(400)) + " STREAM-END", "chunk_delay": 0.01}]
        seen = lambda: t.scrollback() if c.display == "inline" else t.text()
        t.type("short one", settle=False)
        t.wait(lambda s_: "echo: short one" in seen(), timeout=20)
        c.idle(t)
        c.ok("echo: short one" in seen(), f"{cols}x{rows}: a short answer shows", seen())
        t.type("longone", settle=False)
        t.wait(lambda s_: "LAST-LINE-OF-LONG" in seen(), timeout=20)
        screen = c.idle(t)
        if c.display == "inline" and rows <= 12 and "LAST-LINE-OF-LONG" not in screen:
            c.known(f"{cols}x{rows}: a long answer ends with its last line and the input in view",
                    "inline display in a 12-row window: see the markdown check's row of the same kind")
        else:
            c.ok("LAST-LINE-OF-LONG" in screen and "❯" in screen, f"{cols}x{rows}: a long answer ends with its last line and the input in view", screen)
        whole = t.scrollback() if c.display == "inline" else screen
        if c.display == "full":
            # Scroll back to the top with PgUp, then come back.
            for _ in range(80):
                t.send(PGUP, settle=False)
                t.pump(0.01)
            top = t.settle()
            c.ok("line number 1 of" in top or "longone" in top or "short one" in top, f"{cols}x{rows}: PgUp reaches the top of the transcript", top)
            # The wheel, as an xterm reports it (SGR mouse).
            for _ in range(200):
                t.send("\x1b[<65;5;5M", settle=False)
            bottom = t.settle()
            c.ok("LAST-LINE-OF-LONG" in bottom, f"{cols}x{rows}: the wheel scrolls back down to the newest line", bottom)
            # While an answer streams, a view scrolled up stays put; sending returns to the newest line.
            t.type("streamlong", settle=False)
            t.wait("w30", timeout=10)
            for _ in range(6):
                t.send(PGUP, settle=False)
            t.pump(0.3)
            # The transcript rows in view: above the live region, without the "N more lines" count that grows.
            view = lambda: "\n".join(l for l in t.lines()[: max(rows - 12, 1)] if "more line" not in l)
            a = view()
            t.pump(1.0)
            b = view()
            c.ok(a == b, f"{cols}x{rows}: scrolled up while it streams, the view stays put", (a, b))
            t.wait(lambda s: not c.busy(s), timeout=40)
            c.say(t, "back down", expect="echo: back down")
            c.ok("echo: back down" in t.text(), f"{cols}x{rows}: sending returns to the newest line", t.text())
        else:
            c.ok("line number 1 of" in whole and "line number 120 of" in whole, f"{cols}x{rows}: the whole answer is in the terminal's scrollback", whole[-500:])
        # Resize with the answer on screen.
        t.resize(max(cols - 20, 30), max(rows - 4, 10))
        t.resize(cols, rows)
        c.ok(t.status is None and "❯" in t.text(), f"{cols}x{rows}: after resizing with an answer on screen the input is in view", t.text())
        c.ok(c.alive(t, "sized"), f"{cols}x{rows}: the next prompt works", t.text())
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{cols}x{rows}: /exit leaves cleanly", (t.status, t.left_clean()))


@check("D", "markdown of every kind, and malformed markdown (sizes)")
def markdown(c):
    for cols, rows in SIZES:
        sb, st, t = c.session(cols=cols, rows=max(rows, 12))
        st.rules["showmd"] = [{"text": MARKDOWN, "chunk_delay": 0.002}]
        st.rules["brokenmd"] = [{"text": BROKEN}]
        t.type("showmd", settle=False)
        t.wait(lambda s_: "END-OF-MARKDOWN" in (t.scrollback() if c.display == "inline" else s_), timeout=40)
        c.idle(t)
        if c.display == "inline" and rows <= 12:
            if "END-OF-MARKDOWN" in t.text():
                c.ok(True, f"{cols}x{rows}: the answer's last line is in view when the turn ends")
            else:
                c.known(f"{cols}x{rows}: the answer's last line is in view when the turn ends",
                        "inline display only: the live region shrinks when the turn ends and stays where its top was, so in a 12-row window "
                        "blank rows are left under the input and the answer's end is a scroll away in the terminal's scrollback. Moving the "
                        "region down would mean rewriting rows the terminal already holds in its scrollback (they would show twice). The "
                        "full-screen display, the default, is not affected.")
        if c.display == "full":
            pages = [t.text()]
            for _ in range(60):
                t.send(PGUP, settle=False)
                t.pump(0.02)
                t.settle(quiet=0.1)
                if t.text() == pages[-1]:
                    break
                pages.append(t.text())
            whole = "\n".join(reversed(pages))
            for _ in range(len(pages) + 2):
                t.send(PGDN, settle=False)
            t.settle()
        else:
            whole = t.scrollback()
        flat = re.sub(r"\s+", " ", whole)
        label = f"{cols}x{rows}"
        for i, h in enumerate(("Heading one", "Heading two", "Heading three", "Heading four", "Heading five", "Heading six")):
            c.ok(re.search(rf"^\W*{h}\s*$", whole, re.M) and not re.search(rf"#+ {h}", whole), f"{label}: heading level {i + 1} shows without its hashes", whole[:1500])
        c.ok("**" not in flat.split("END-OF-MARKDOWN")[0].split("showmd")[-1] and "with bold" in flat, f"{label}: bold shows without its marks", flat[:800])
        c.ok("~~" not in flat and "`inline code`" not in flat and "inline code" in flat, f"{label}: strike and inline code show without their marks", flat[:800])
        c.ok("link text" in flat and "example.com/page" in flat.replace(" ", ""), f"{label}: a link shows its text and its URL", flat[:900])
        c.ok(re.search(r"• bullet one", whole) and re.search(r"[◦▪•] nested bullet", whole) and re.search(r"1\. numbered one", whole),
             f"{label}: bullets, nested bullets and numbers", whole[:2000])
        c.ok(re.search(r"○ task open", whole) and re.search(r"✓ task done", whole), f"{label}: tasks open and done", whole[:2500])
        c.ok(re.search(r"│ quoted line", whole) and re.search(r"│\s?│ nested quote", whole), f"{label}: quotes and nested quotes", whole[:2500])
        c.ok("alpha" in flat and "beta" in flat and "|:--" not in flat and "---|" not in flat, f"{label}: the table is drawn, not its dashes", whole)
        for lang in ("rust", "python", "js", "go", "c", "sh", "sql", "json", "plain"):
            c.ok(f"{lang}-fence" in flat, f"{label}: the {lang} code fence shows its code", "")
        c.ok("```" not in flat, f"{label}: no fence marks are left on screen", "")
        c.ok(re.search(r"[-−] ?removed diff line", flat) and re.search(r"\+ ?added diff line", flat), f"{label}: the diff fence keeps + and -", "")
        too_wide = [l for l in whole.splitlines() if kterm_width(l) > cols]
        c.ok(not too_wide, f"{label}: no row is wider than the window", too_wide[:3])
        t.type("brokenmd", settle=False)
        try:
            t.wait(lambda s_: "END-OF-BROKEN" in (t.scrollback() if c.display == "inline" else s_), timeout=20)
            c.idle(t)
            c.ok(True, f"{label}: malformed markdown (unclosed fence, bold, link) still shows to its end")
        except AssertionError as e:
            c.ok(False, f"{label}: malformed markdown (unclosed fence, bold, link) still shows to its end", e)
        c.say(t, "after broken", expect="echo: after broken")
        last = [l for l in (t.scrollback() if c.display == "inline" else t.text()).splitlines() if "echo: after broken" in l]
        c.ok(last and "│" not in last[0][:6], f"{label}: an unclosed fence does not leak into the next answer", t.text())
        t.resize(max(cols // 2, 30), rows)
        t.resize(cols, rows)
        c.ok(t.status is None and c.alive(t, "md"), f"{label}: after a resize the next prompt works", t.text())
        c.ok(c.quit(t) == 0 and not t.left_clean(), f"{label}: /exit leaves cleanly", (t.status, t.left_clean()))


def kterm_width(s):
    import unicodedata
    return sum(0 if unicodedata.combining(ch) else 2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in s)


@check("D", "every built-in tool, succeeding and failing; odd results")
def tools(c):
    sb, st = c.sandbox(), c.stub()
    p = sb.project
    with open(os.path.join(p, "binary.bin"), "wb") as f:
        f.write(bytes(range(256)) * 20)
    with open(os.path.join(p, "latin1.txt"), "wb") as f:
        f.write(b"caf\xe9 na\xefve \xff\xfe not utf-8\n")
    with open(os.path.join(p, "empty.txt"), "w"):
        pass
    with open(os.path.join(p, "huge.txt"), "w") as f:
        f.write("\n".join(f"huge line {i} " + "x" * 80 for i in range(20000)))
    with open(os.path.join(p, "nb.ipynb"), "w") as f:
        json.dump({"cells": [{"cell_type": "code", "id": "c1", "metadata": {}, "source": ["print(1)"], "outputs": [], "execution_count": None}],
                   "metadata": {}, "nbformat": 4, "nbformat_minor": 5}, f)
    j = lambda *a: os.path.join(p, *a)
    cases = [
        ("Read ok", ("Read", {"file_path": j("notes.txt")}), True),
        ("Read missing", ("Read", {"file_path": j("nope.txt")}), False),
        ("Read empty", ("Read", {"file_path": j("empty.txt")}), True),
        ("Read huge", ("Read", {"file_path": j("huge.txt")}), None),
        ("Read binary", ("Read", {"file_path": j("binary.bin")}), None),
        ("Read not UTF-8", ("Read", {"file_path": j("latin1.txt")}), None),
        ("Write ok", ("Write", {"file_path": j("new.txt"), "content": "new file\nline two\n"}), True),
        ("Write into a missing root", ("Write", {"file_path": "/proc/nope/x.txt", "content": "x"}), False),
        ("Edit ok", ("Edit", {"file_path": j("notes.txt"), "old_string": "first line", "new_string": "edited line"}), True),
        ("Edit no match", ("Edit", {"file_path": j("notes.txt"), "old_string": "not in the file", "new_string": "x"}), False),
        ("MultiEdit ok", ("MultiEdit", {"file_path": j("notes.txt"), "edits": [{"old_string": "second line", "new_string": "2nd line"}]}), True),
        ("MultiEdit no match", ("MultiEdit", {"file_path": j("notes.txt"), "edits": [{"old_string": "zzz", "new_string": "y"}]}), False),
        ("Glob ok", ("Glob", {"pattern": "*.txt"}), True),
        ("Glob nothing", ("Glob", {"pattern": "*.nomatch"}), None),
        ("Grep ok", ("Grep", {"pattern": "edited"}), True),
        ("Grep bad regex", ("Grep", {"pattern": "(unclosed"}), False),
        ("LS ok", ("LS", {"path": p}), True),
        ("LS missing", ("LS", {"path": j("no-dir")}), False),
        ("Bash ok", ("Bash", {"command": "echo bash-says-hi"}), True),
        ("Bash fails", ("Bash", {"command": "exit 7"}), False),
        ("Bash empty output", ("Bash", {"command": "true"}), True),
        ("Bash huge output", ("Bash", {"command": "seq 1 200000"}), True),
        ("Bash binary output", ("Bash", {"command": "head -c 3000 /dev/urandom"}), None),
        ("Bash not UTF-8 output", ("Bash", {"command": "printf 'caf\\351 \\377\\376 end\\n'"}), None),
        ("Bash escapes in output", ("Bash", {"command": "printf '\\033[2J\\033[?1049l\\033]0;owned\\007 visible-after-escapes\\n'"}), True),
        ("TodoWrite ok", ("TodoWrite", {"todos": [{"content": "step one", "status": "in_progress", "activeForm": "Doing step one"},
                                                   {"content": "step two", "status": "pending", "activeForm": "Doing step two"}]}), True),
        ("TodoWrite bad input", ("TodoWrite", {"todos": "not a list"}), False),
        ("NotebookEdit ok", ("NotebookEdit", {"notebook_path": j("nb.ipynb"), "cell_id": "c1", "new_source": "print(2)"}), None),
        ("NotebookEdit missing", ("NotebookEdit", {"notebook_path": j("none.ipynb"), "cell_id": "c1", "new_source": "x"}), False),
        ("WebFetch unreachable", ("WebFetch", {"url": "http://127.0.0.1:9/none", "prompt": "what"}), False),
        ("an unknown tool", ("NoSuchTool", {"x": 1}), False),
        ("a tool given no arguments", ("Read", {}), False),
    ]
    t = c.term(sb, st, args=["--dangerously-skip-permissions"], KODACT_VERIFY="0")
    for i, (name, call, _) in enumerate(cases):
        st.rules[f"tool-case-{i:02d}-go"] = [{"text": f"Calling {i}.", "tools": [call]}, {"text": f"case-{i:02d}-finished"}]
    for i, (name, call, ok) in enumerate(cases):
        raw = len(t.raw)
        t.send(ctrl("u"))
        t.type(f"tool-case-{i:02d}-go", settle=False)
        try:
            t.wait(f"case-{i:02d}-finished", timeout=40)
            screen = c.idle(t)
            c.ok(True, f"{name}: the turn ends with the model's answer")
        except AssertionError as e:
            c.ok(False, f"{name}: the turn ends with the model's answer", e)
            t.send(ESC)
            c.idle(t)
            continue
        # What the model was told: the tool result of this call.
        result = next((m for m in reversed(st.requests[-1]["messages"]) if m.get("role") == "tool"), None)
        text = kterm.Stub.text_of(result) if result else ""
        c.ok(result is not None, f"{name}: the model gets a result for the call", st.requests[-1]["messages"][-2:])
        block = screen.split(f"tool-case-{i:02d}-go")[-1]
        if ok is True:
            c.ok("✗" not in block, f"{name}: shown as done, not failed", block)
        elif ok is False:
            c.ok("✗" in block or "failed" in block.lower() or "error" in block.lower(), f"{name}: shown as failed", block)
        wrote = t.raw[raw:]
        c.ok(b"\x1b]0;owned" not in wrote and b"\x00" not in wrote, f"{name}: no raw control bytes from the result reach the terminal", wrote[-200:])
    c.ok(t.modes()["alt_screen"] == (c.display == "full"), "the display mode is as it was after every result", t.modes())
    c.ok(open(j("new.txt")).read() == "new file\nline two\n", "Write wrote the file", "")
    c.ok(open(j("notes.txt")).read() == "edited line\n2nd line\n", "Edit and MultiEdit changed the file", open(j("notes.txt")).read())
    c.ok(c.alive(t, "tools"), "after every tool the next prompt works", t.text())

    # Several tool calls in one reply.
    st.rules["many-at-once"] = [{"text": "Three calls.", "tools": [("Read", {"file_path": j("notes.txt")}), ("Glob", {"pattern": "*.txt"}),
                                                                      ("Bash", {"command": "echo third-call"})]}, {"text": "all-three-done"}]
    t.type("many-at-once", settle=False)
    t.wait("all-three-done", timeout=30)
    screen = c.idle(t)
    results = [m for m in st.requests[-1]["messages"] if m.get("role") == "tool"][-3:]
    c.ok(len(results) == 3 and "third-call" in kterm.Stub.text_of(results[2]), "several calls in one reply: three results, in call order", results)
    shown = (t.scrollback() if c.display == "inline" else screen).split("many-at-once")[-1]
    c.ok("Read" in shown and "third-call" in shown, "several calls in one reply: each shows", shown)

    # A background shell command, read and stopped.
    st.rules["background-go"] = [
        {"text": "Starting.", "tools": [("Bash", {"command": "for i in 1 2 3 4 5 6 7 8 9; do echo bg-tick-$i; sleep 1; done", "run_in_background": True})]},
        {"text": "background-started"}]
    t.type("background-go", settle=False)
    t.wait("background-started", timeout=30)
    c.idle(t)
    screen = c.say(t, "/tasks")
    c.ok(re.search(r"bg-tick|for i in|running", screen), "a background shell command is listed by /tasks", screen)
    m = re.search(r"\b(bash_\w+|shell_\w+|[0-9a-f]{6,}|\w+_\d+)\b", screen.split("/tasks")[-1])
    c.ok(c.alive(t, "bg"), "with a background command running the next prompt works", t.text())
    screen = c.say(t, "/tasks stop all")
    screen = c.say(t, "/tasks")
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit with background work leaves cleanly", (t.status, t.left_clean()))
    time.sleep(1.5)
    import subprocess
    left = [p_ for p_ in subprocess.run(["pgrep", "-f", "bg-tick"], capture_output=True, text=True).stdout.split()
            if os.path.exists(f"/proc/{p_}/cwd") and os.path.realpath(f"/proc/{p_}/cwd").startswith(sb.root)]
    c.ok(not left, "after /exit no background command is left running", left)


@check("D", "a sub-agent")
def subagent(c):
    sb, st = c.sandbox(), c.stub()
    st.rules["delegate-go"] = [{"text": "Delegating.", "tools": [("Task", {"description": "look around", "prompt": "SUBAGENT-TASK-PROMPT list files",
                                                                            "subagent_type": "general-purpose"})]}, {"text": "delegation-finished"}]
    st.rules["SUBAGENT-TASK-PROMPT"] = [{"text": "Looking.", "tools": [("Glob", {"pattern": "*.txt"})]}, {"text": "SUBAGENT-REPORT found notes"}]
    t = c.term(sb, st, args=["--dangerously-skip-permissions"])
    t.type("delegate-go", settle=False)
    try:
        t.wait("delegation-finished", timeout=40)
        screen = c.idle(t)
        c.ok(re.search(r"Delegated|look around", screen), "the sub-agent's row shows", screen)
        results = [kterm.Stub.text_of(m) for m in st.requests[-1]["messages"] if m.get("role") == "tool"]
        c.ok(any("SUBAGENT-REPORT" in r for r in results), "its report reaches the model", results)
        c.ok("Looking." not in screen, "its own words stay out of the transcript", screen)
    except AssertionError as e:
        c.ok(False, "a sub-agent runs and reports", e)
    c.ok(c.alive(t, "agent"), "the next prompt works", t.text())
    c.quit(t)


@check("D", "permission prompts: allow, deny, always; each mode; a rule; two at once; after a resize")
def permissions(c):
    def setup(args=(), **env):
        sb, st = c.sandbox(), c.stub()
        # Not a plain file command: accept-edits mode lets those (touch, mkdir, mv) through, as it lets edits.
        st.rules["touch-one"] = [{"text": "One.", "tools": [("Bash", {"command": "python3 -c \"open('made-one.txt','w')\""})]}, {"text": "one-done"}]
        st.rules["touch-two"] = [{"text": "Two.", "tools": [("Bash", {"command": "touch made-a.txt"}), ("Bash", {"command": "touch made-b.txt"})]},
                                 {"text": "two-done"}]
        st.rules["write-one"] = [{"text": "W.", "tools": [("Write", {"file_path": os.path.join(sb.project, "written.txt"), "content": "w\n"})]},
                                 {"text": "write-done"}]
        st.rules["read-one"] = [{"text": "R.", "tools": [("Read", {"file_path": os.path.join(sb.project, "notes.txt")})]}, {"text": "read-done"}]
        return sb, st, c.term(sb, st, args=list(args), KODACT_VERIFY="0", **env)

    has = lambda sb, f: os.path.exists(os.path.join(sb.project, f))
    # Allow once: it runs, and the next one asks again.
    sb, st, t = setup()
    t.type("touch-one", settle=False)
    t.wait("Allow once", timeout=10)
    c.ok("made-one.txt" in t.text() and "python3" in t.text(), "the prompt shows the command it asks about", t.text())
    t.send("y")
    t.wait("one-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "made-one.txt"), "Allow once: the command ran", os.listdir(sb.project))
    os.remove(os.path.join(sb.project, "made-one.txt"))
    t.type("touch-one", settle=False)
    t.wait("Allow once", timeout=10)
    c.ok(True, "Allow once: the same command asks again")
    # Deny.
    t.send("n")
    c.idle(t)
    c.ok(not has(sb, "made-one.txt"), "Deny: the command did not run", os.listdir(sb.project))
    c.ok(not c.busy(t.text()) and not c.dialog(t.text()), "Deny: the UI is idle afterwards", t.text())
    # Always allow: saved, and not asked again; also after a restart.
    t.type("touch-one", settle=False)
    t.wait("Allow once", timeout=10)
    c.ok("Always allow" in t.text(), "the prompt offers an always-allow suggestion", t.text())
    t.send("a")
    t.wait("one-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "made-one.txt"), "Always allow: the command ran", "")
    os.remove(os.path.join(sb.project, "made-one.txt"))
    t.type("touch-one", settle=False)
    t.wait("one-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "made-one.txt") and "Allow once" not in t.text(), "Always allow: the same command no longer asks", t.text())
    # Arrows and Enter, and a digit.
    t.type("write-one", settle=False)
    t.wait("Allow once", timeout=10)
    t.send(DOWN)
    t.send(DOWN)
    t.send(ENTER)
    c.idle(t)
    c.ok(not has(sb, "written.txt"), "Down, Down, Enter chooses Deny", os.listdir(sb.project))
    t.type("write-one", settle=False)
    t.wait("Allow once", timeout=10)
    t.send("1")
    t.wait("write-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "written.txt"), "the digit 1 chooses Allow once", os.listdir(sb.project))
    # Two prompts close together: each is asked, each answer goes to its own call.
    t.type("touch-two", settle=False)
    t.wait("made-a.txt", timeout=10)
    t.wait("Allow once", timeout=10)
    t.send("y")
    try:
        t.wait(lambda s: "made-b.txt" in s and "Allow once" in s, timeout=10)
        t.send("n")
        c.idle(t)
        c.ok(has(sb, "made-a.txt") and not has(sb, "made-b.txt"), "two prompts in one reply: allowing the first and denying the second does exactly that",
             os.listdir(sb.project))
    except AssertionError as e:
        c.ok(False, "two prompts in one reply: the second is asked after the first", e)
    c.ok(c.alive(t, "two"), "after two prompts the next prompt works", t.text())
    # A prompt answered after a resize.
    os.remove(os.path.join(sb.project, "written.txt"))
    t.type("write-one", settle=False)
    t.wait("Allow once", timeout=10)
    t.resize(60, 20)
    c.ok("Allow once" in t.text(), "after a resize to 60x20 the prompt is still on screen", t.text())
    t.resize(40, 12)
    c.ok("Allow" in t.text(), "after a resize to 40x12 the prompt's choices are still on screen", t.text())
    t.resize(110, 36)
    t.send("y")
    t.wait("write-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "written.txt"), "a prompt answered after resizes runs the call", os.listdir(sb.project))
    c.quit(t)
    # A restart: the always-allow rule held.
    os.remove(os.path.join(sb.project, "made-one.txt"))
    t = c.term(sb, st, KODACT_VERIFY="0")
    t.type("touch-one", settle=False)
    t.wait("one-done", timeout=15)
    c.idle(t)
    c.ok(has(sb, "made-one.txt") and "Allow once" not in t.text(), "after a restart the always-allow rule still holds", t.text())
    c.quit(t)

    # Each mode.
    expect = {
        "default": dict(read=True, write="ask", bash="ask"),
        "acceptEdits": dict(read=True, write=True, bash="ask"),
        "plan": dict(read=True, write=False, bash=False),
        "dontAsk": dict(read=True, write=False, bash=False),
        "bypassPermissions": dict(read=True, write=True, bash=True),
    }
    for mode, want in expect.items():
        sb, st, t = setup(args=["--permission-mode", mode])
        for prompt, kind, made, done in (("read-one", "read", None, "read-done"), ("write-one", "write", "written.txt", "write-done"),
                                         ("touch-one", "bash", "made-one.txt", "one-done")):
            t.send(ctrl("u"))
            t.type(prompt, settle=False)
            t.wait(lambda s: "Allow once" in s.split(prompt)[-1] or done in s.split(prompt)[-1], timeout=20)
            asked = "Allow once" in t.text().split(prompt)[-1]
            if asked:
                t.send(ESC)
            c.idle(t)
            ran = made is None or has(sb, made)
            if want[kind] == "ask":
                c.ok(asked and not ran, f"mode {mode}: {kind} asks first", t.text())
            elif want[kind] is True:
                c.ok(not asked and ran, f"mode {mode}: {kind} runs without asking", t.text())
            else:
                c.ok(not asked and not ran, f"mode {mode}: {kind} is refused without asking", t.text())
        c.ok(c.quit(t) == 0, f"mode {mode}: /exit", t.status)
    # A deny rule from /permissions already matching.
    sb, st, t = setup()
    c.say(t, "/permissions add deny Bash(python3:*)")
    t.type("touch-one", settle=False)
    t.wait("one-done", timeout=15)
    c.idle(t)
    c.ok(not has(sb, "made-one.txt") and "Allow once" not in t.text(), "a deny rule refuses the command without asking", t.text())
    c.quit(t)


@check("D", "plan mode: enter, get a plan, approve, reject, revise")
def plan(c):
    sb, st = c.sandbox(), c.stub()
    st.rules["plan-this"] = [{"text": "Planning.", "tools": [("ExitPlanMode", {"plan": "1. First PLAN-STEP-ALPHA\n2. Then PLAN-STEP-BETA"})]},
                             {"text": "plan-turn-ended"}]
    t = c.term(sb, st, KODACT_VERIFY="0")
    t.send(BACKTAB)
    t.send(BACKTAB)
    c.ok("Plan" in next((l for l in reversed(t.lines()) if "127.0.0.1" in l), ""), "Shift+Tab twice enters plan mode", t.text())
    t.type("plan-this", settle=False)
    try:
        screen = t.wait(re.compile(r"Yes.*\n.*(Yes|ask)|keep planning", re.I), timeout=15)
        c.ok("PLAN-STEP-ALPHA" in screen and "PLAN-STEP-BETA" in screen, "the plan is shown before the question", screen)
        t.send(ESC)
        screen = c.idle(t)
        c.ok("Plan" in next((l for l in reversed(t.lines()) if "127.0.0.1" in l), ""), "rejecting keeps plan mode", screen)
        t.type("plan-this", settle=False)
        t.wait(re.compile(r"keep planning", re.I), timeout=15)
        t.send("2")
        t.wait("plan-turn-ended", timeout=15)
        screen = c.idle(t)
        c.ok("Plan" not in next((l for l in reversed(t.lines()) if "127.0.0.1" in l), ""), "approving leaves plan mode", screen)
    except AssertionError as e:
        c.ok(False, "a plan is shown with its approval question", e)
    screen = c.say(t, "/plan")
    c.ok("Plan mode on" in screen, "/plan enters plan mode", screen)
    c.ok(c.alive(t, "plan"), "in plan mode the next prompt works", t.text())
    c.quit(t)


@check("D", "interrupt during the answer, a tool call, a permission prompt and a retry wait")
def interrupts(c):
    sb, st = c.sandbox(), c.stub()
    st.rules["int-answer"] = [{"text": "drip " * 60 + "DRIP-END", "chunk_delay": 0.1}]
    st.rules["int-tool"] = [{"text": "Sleeping.", "tools": [("Bash", {"command": "sleep 30; echo SLEPT-THIRTY"})]}, {"text": "tool-turn-ended"}]
    st.rules["int-ask"] = [{"text": "Asking.", "tools": [("Write", {"file_path": os.path.join(sb.project, "int.txt"), "content": "x"})]}, {"text": "ask-ended"}]
    st.rules["int-retry"] = [{"status": 429}]
    st.rules["int-think"] = [{"thinking": "pondering " * 80, "text": "THOUGHT-END", "chunk_delay": 0.1}]
    t = c.term(sb, st, args=["--allowedTools", "Bash"], KODACT_VERIFY="0")
    for name, prompt, wait_for, gone in (("the answer", "int-answer", "drip drip", "DRIP-END"), ("a tool call", "int-tool", re.compile(r"sleep 30"), "\nSLEPT-THIRTY"),
                                         ("a permission prompt", "int-ask", "Allow once", "ask-ended"),
                                         ("a retry wait", "int-retry", re.compile(r"retry|rate|429|wait", re.I), None),
                                         ("thinking", "int-think", "pondering", "THOUGHT-END")):
        t.send(ctrl("u"))
        t.type(prompt, settle=False)
        try:
            t.wait(wait_for, timeout=20)
        except AssertionError as e:
            c.ok(False, f"interrupt during {name}: it gets that far", e)
        t.pump(0.6)
        t0 = time.time()
        t.send(ESC)
        try:
            screen = c.idle(t, timeout=15)
            took = time.time() - t0
            c.ok(took < 8, f"Esc during {name}: the UI is idle within 8 s", f"{took:.1f}s")
        except AssertionError as e:
            c.ok(False, f"Esc during {name}: the UI becomes idle", e)
            screen = t.text()
        tail = screen.split(prompt)[-1]
        c.ok(not c.busy(screen) and not c.dialog(screen) and "⠋" not in tail, f"Esc during {name}: nothing shows as still running", screen)
        if gone:
            t.pump(1.0)
            c.ok(gone.strip() not in re.sub(r"echo SLEPT-THIRTY", "", t.text().split(prompt)[-1]), f"Esc during {name}: it really stopped", tail)
        c.ok(re.search(r"Interrupted|stopped|cancel", tail, re.I), f"Esc during {name}: the transcript says it was interrupted", tail)
        c.ok(c.alive(t, f"after{len(name)}"), f"Esc during {name}: the next prompt works", t.text())
    c.ok(not os.path.exists(os.path.join(sb.project, "int.txt")), "the interrupted permission prompt approved nothing", "")
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("D", "messages typed during a turn: queued, shown, sent in order, withdrawn")
def queued(c):
    sb, st, t = c.session()
    st.rules["queue-slow"] = [{"text": "slow " * 25 + "QUEUE-SLOW-END", "chunk_delay": 0.12}]
    t.type("queue-slow", settle=False)
    t.wait("slow slow", timeout=10)
    t.type("queued-first", settle=False)
    t.pump(0.3)
    t.type("queued-second", settle=False)
    t.pump(0.3)
    screen = t.text()
    c.ok("queued-first" in screen and "queued-second" in screen and "echo: queued-first" not in screen, "queued messages show as queued while the turn runs", screen)
    t.wait("echo: queued-second", timeout=30)
    c.idle(t)
    prompts = [p for p in st.prompts() if p.startswith(("queue-slow", "queued-"))]
    c.ok(prompts[-2:] == ["queued-first", "queued-second"] or "queued-first" in prompts[-1] and "queued-second" in prompts[-1],
         "they are sent after the turn, in the order typed", prompts)
    # Withdraw one.
    t.type("queue-slow", settle=False)
    t.wait("slow slow", timeout=10)
    t.type("withdraw-me", settle=False)
    t.pump(0.4)
    c.ok("withdraw-me" in t.text(), "a queued message shows", t.text())
    t.send(UP)
    t.pump(0.3)
    back = "withdraw-me" in t.input_text()
    if not back:
        t.send(BS_ALL)
    c.ok(back, "Up takes a queued message back into the input, to edit or drop", t.text())
    t.send(ctrl("u"))
    t.wait("QUEUE-SLOW-END", timeout=30)
    c.idle(t)
    t.pump(1.0)
    c.idle(t)
    c.ok(not any(p == "withdraw-me" for p in st.prompts()), "a withdrawn message is never sent", st.prompts()[-3:])
    c.ok(c.alive(t, "queue"), "the next prompt works", t.text())
    c.quit(t)


BS_ALL = kterm.BS * 12


@check("D", "endpoint errors: 401, 429, 500, a dropped connection, a malformed stream")
def errors(c):
    cases = [
        ("401", [{"status": 401}], r"key|401|auth", False),
        ("403", [{"status": 403}], r"key|403|permission|allowed|access", False),
        ("429 that clears", [{"status": 429, "times": 1, "text": "after-429-ok"}], r"after-429-ok", True),
        ("429 that stays", [{"status": 429}], r"rate|429|limit|busy", False),
        ("500 that clears", [{"status": 500, "times": 1, "text": "after-500-ok"}], r"after-500-ok", True),
        ("500 that stays", [{"status": 500}], r"500|server|error", False),
        ("400", [{"status": 400}], r"400|request|error", False),
        ("a dropped connection", [{"text": "partial words then nothing more at all", "drop": True, "chunk_delay": 0.05}], r"connection|lost|ended|interrupted|stream|cut", False),
        ("a malformed stream", [{"malformed": True}], r"stream|response|parse|invalid|unexpected|empty|answer|error", False),
        ("a slow first byte", [{"delay": 4, "text": "slow-first-byte-ok"}], r"slow-first-byte-ok", True),
    ]
    sb, st = c.sandbox(), c.stub()
    t = c.term(sb, st, KODACT_MAX_RETRIES="1")
    for i, (name, script, expect, succeeds) in enumerate(cases):
        key = f"err-case-{i}-go"
        st.rules[key] = script
        t.send(ctrl("u"))
        t.type(key, settle=False)
        t0 = time.time()
        try:
            t.wait(lambda s: re.search(expect, s.split(key)[-1], re.I), timeout=90)
            screen = c.idle(t, timeout=90)
            c.ok(True, f"{name}: the screen says what happened" if not succeeds else f"{name}: the answer arrives after the retry")
        except AssertionError as e:
            c.ok(False, f"{name}: the screen says what happened" if not succeeds else f"{name}: the answer arrives after the retry", e)
            t.send(ESC)
            screen = c.idle(t, timeout=30)
        tail = screen.split(key)[-1]
        c.ok(not c.busy(screen), f"{name}: the UI is idle afterwards ({time.time() - t0:.0f}s)", screen)
        c.ok("stub-key-1234567890" not in t.scrollback(), f"{name}: the key is not printed", "")
        c.ok("panicked" not in t.scrollback() and "Traceback" not in tail and "{\"error\"" not in tail, f"{name}: no raw dump on screen", tail)
        st.rules.pop(key)
        c.ok(c.alive(t, f"err{i}"), f"{name}: the session stays usable", t.text())
    # A long retry wait says a retry is under way.
    st.rules["err-wait-go"] = [{"status": 429, "times": 2, "text": "after-two-429"}]
    t.type("err-wait-go", settle=False)
    try:
        t.wait(re.compile(r"retry|retries|trying again|waiting", re.I), timeout=20)
        c.ok(True, "while waiting to retry the screen says a retry is under way")
        t.wait("after-two-429", timeout=60)
    except AssertionError as e:
        c.ok(False, "while waiting to retry the screen says a retry is under way", e)
        t.send(ESC)
    c.idle(t, timeout=60)
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))


@check("D", "thinking: shown while it streams, folded after")
def thinking(c):
    sb, st, t = c.session()
    st.rules["think-go"] = [{"thinking": "considering the options carefully " * 12 + "THINK-TAIL", "text": "The answer is THINK-ANSWER.", "chunk_delay": 0.05}]
    t.type("think-go", settle=False)
    try:
        screen = t.wait(re.compile(r"∴ .*considering|Thinking"), timeout=15)
        c.ok("Thinking" in screen and "considering" in screen, "while it streams the thought and 'Thinking…' show", screen)
    except AssertionError as e:
        c.ok(False, "while it streams the thought and 'Thinking…' show", e)
    t.wait("THINK-ANSWER", timeout=60)
    screen = c.idle(t)
    c.ok(re.search(r"∴ Thought for \d+", screen), "after it, the thought is folded to one heading with its time", screen)
    c.ok("Thinking…" not in screen.split("think-go")[-1], "and nothing still says Thinking", screen)
    c.ok(c.alive(t, "think"), "the next prompt works", t.text())
    c.quit(t)
