"""E. Long sessions and limits."""

import json
import os
import re
import time

import kterm
from kterm import ESC, PGDN, PGUP, ctrl
from run import check


def rss_kb(pid):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    except OSError:
        pass
    return 0


@check("E", "a session of 200 turns: memory, typing, scrolling, the meter")
def two_hundred(c):
    sb, st, t = c.session()
    st.rules["bulk"] = [{"text": "\n".join(f"reply line {i} with some words in it" for i in range(12))}]
    marks = {}
    for i in range(1, 201):
        t.type(f"bulk turn {i:03d}", settle=False)
        t.wait(lambda s, i=i: f"bulk turn {i:03d}" in s and not c.busy(s) and "reply line 11" in s.split(f"bulk turn {i:03d}")[-1], timeout=30)
        if i in (20, 100, 200):
            t.settle()
            marks[i] = rss_kb(t.pid)
    c.ok(t.status is None, "200 turns: the program is still running", t.scrollback()[-500:])
    grow = marks[200] - marks[100]
    c.ok(marks[200] < marks[20] + 120_000 and grow < 80_000, "memory stays level (under 120 MB more than at turn 20)", marks)
    # Typing stays responsive: 40 characters are on screen within 2 s.
    t0 = time.time()
    word = "responsive-typing-check-0123456789-abcde"
    for ch in word:
        os.write(t.fd, ch.encode())
    t.wait(word, timeout=5)
    took = time.time() - t0
    c.ok(took < 2.0, "typing 40 characters shows within 2 s after 200 turns", f"{took:.2f}s")
    t.send(ctrl("u"))
    if c.display == "full":
        t0 = time.time()
        for _ in range(400):
            t.send(PGUP, settle=False)
            if _ % 20 == 19:
                t.pump(0.05)
        top = t.settle(timeout=20)
        c.ok("bulk turn 001" in top, "PgUp reaches the first turn", top)
        for _ in range(400):
            t.send(PGDN, settle=False)
            if _ % 20 == 19:
                t.pump(0.05)
        bottom = t.settle(timeout=20)
        c.ok("bulk turn 200" in bottom, "PgDn returns to the newest turn", bottom)
        c.ok(time.time() - t0 < 60, "scrolling to the top and back takes under a minute", f"{time.time() - t0:.0f}s")
    else:
        c.ok("bulk turn 001" in t.scrollback() and "bulk turn 200" in t.scrollback(), "every turn is in the terminal's scrollback", "")
    # The meter against /context.
    t.type("/context", settle=False)
    screen = t.wait(re.compile(r"about [\d,]+ of [\d,]+ tokens"), timeout=15)
    m = re.search(r"about ([\d,]+) of ([\d,]+) tokens \(([\d.]+)%\)", screen)
    t.send(ESC)
    t.settle()
    panel = next((l for l in reversed(t.lines()) if "% left" in l), "")
    left = re.search(r"(\d+)% left", panel)
    if m and left:
        used_pct = float(m.group(3))
        c.ok(abs((100 - int(left.group(1))) - used_pct) <= 6 or int(left.group(1)) <= 100 - used_pct + 30,
             "the context meter agrees with /context within a few points", (panel, m.group(0)))
    else:
        c.ok(False, "the context meter and /context both show a figure", (panel, t.text()))
    sent = st.requests[-1]["messages"]
    c.ok(any("bulk turn 001" in kterm.Stub.text_of(m_) for m_ in sent) or any("summary" in kterm.Stub.text_of(m_).lower() for m_ in sent),
         "the last request still carries the first turn (or a summary of it)", len(sent))
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))
    # And it resumes.
    t = c.term(sb, st, args=["--continue"])
    c.ok("bulk turn 200" in t.text(), "--continue shows the 200-turn conversation's end", t.text())
    c.ok(c.alive(t, "resumed"), "and takes the next prompt", t.text())
    c.quit(t)


@check("E", "auto-compaction on a small window and none on a large one")
def autocompact(c):
    fact = "The launch code word is PERSIMMON-7781."
    filler = "filler words to take up room in the context window " * 60  # about 800 tokens
    for window, should in ((20000, True), (1000000, False)):
        sb, st = c.sandbox(), c.stub(window=window)
        t = c.term(sb, st)
        st.rules["fill"] = [{"text": filler}]
        c.say(t, fact + " Remember it.", expect="echo:")
        compacted_at = None
        for i in range(30):
            sent = len(st.requests)
            t.type(f"fill {i:02d} " + "pad " * 150, settle=False)
            t.wait(lambda s, sent=sent: len(st.requests) > sent and not c.busy(s), timeout=40)
            c.idle(t, timeout=40)
            if "ompacted" in t.scrollback() + t.text():
                compacted_at = i
                break
        label = f"window {window:,}"
        if should:
            c.ok(compacted_at is not None, f"{label}: the conversation is compacted automatically", t.text())
            c.ok(re.search(r"Conversation compacted|compacted", t.scrollback() + t.text()), f"{label}: the screen says so", t.text())
            summaries = [r for r in st.requests if "is about to be replaced by a summary" in json.dumps(r["messages"][-1])]
            if summaries:
                size = len(json.dumps(summaries[0]["messages"])) // 4
                c.ok(0.4 * window < size < 1.05 * window, f"{label}: it happens near the window, not long before (at about {size:,} tokens)", size)
            c.say(t, "what was the code word", expect="echo:")
            sent = json.dumps(st.requests[-1]["messages"])
            c.ok("PERSIMMON-7781" in sent, f"{label}: the person's words survive the compaction word for word", sent[:600])
            c.ok(len(sent) // 4 < window, f"{label}: the request after it fits the window", len(sent) // 4)
            panel = next((l for l in reversed(t.lines()) if "% left" in l), "")
            c.ok(re.search(r"\d+% left", panel), f"{label}: the meter shows the room left after it", panel)
        else:
            c.ok(compacted_at is None, f"{label}: nothing is compacted in 30 turns", t.text()[-800:])
            c.ok(not any("is about to be replaced by a summary" in json.dumps(r["messages"][-1]) for r in st.requests),
                 f"{label}: no summary request is made", "")
        c.ok(c.alive(t, "ac"), f"{label}: the next prompt works", t.text())
        c.quit(t)


@check("E", "/loop, /tasks, /subtask, /goal: start, list, finish, stop, /clear while one runs")
def background(c):
    sb, st = c.sandbox(), c.stub()
    st.rules["SUBTASK-JOB"] = [{"text": "SUBTASK-REPORT all done", "delay": 2}]
    st.rules["SLOW-SUBTASK"] = [{"text": "slow report", "delay": 25}]
    t = c.term(sb, st)
    c.say(t, "seed", expect="echo: seed")
    # /subtask: start, list, let it finish.
    screen = c.say(t, "/subtask SUBTASK-JOB please")
    c.ok(t.status is None and re.search(r"subtask|background|started", screen, re.I), "/subtask starts and says so", screen)
    screen = c.say(t, "/tasks")
    c.ok(re.search(r"SUBTASK-JOB|subtask", screen, re.I), "/tasks lists the running subtask", screen)
    try:
        t.wait(re.compile(r"has finished"), timeout=30)
        c.ok(True, "the subtask finishes and the screen says so")
    except AssertionError as e:
        c.ok(False, "the subtask finishes and the screen says so", e)
    c.idle(t)
    c.say(t, "what did it find", expect="echo:")
    c.ok("SUBTASK-REPORT" in json.dumps(st.requests[-1]["messages"]), "its report reaches the model with the next prompt", json.dumps(st.requests[-1]["messages"])[-600:])
    # /loop on an interval: listed, then stopped.
    screen = c.say(t, "/loop 1h LOOP-PROMPT check things", expect="echo:")
    c.ok(any("LOOP-PROMPT" in p for p in st.prompts()), "/loop runs the prompt now", st.prompts()[-2:])
    screen = c.say(t, "/tasks")
    c.ok("LOOP-PROMPT" in screen or re.search(r"every|1h|hour", screen), "/tasks lists the scheduled loop", screen)
    ids = re.findall(r"^\s*(?:[-•*]\s*)?([A-Za-z0-9_-]{4,})\b.*LOOP-PROMPT", screen.split("/tasks")[-1], re.M)
    screen = c.say(t, "/tasks stop " + (ids[-1] if ids else "all"))
    after = c.say(t, "/tasks")
    c.ok("LOOP-PROMPT" not in after.split("/tasks")[-1], "/tasks stop ends it", (ids, screen, after))
    screen = c.say(t, "/tasks stop no-such-id")
    c.ok(t.status is None and re.search(r"no |not|unknown", screen, re.I), "/tasks stop with a wrong id says so", screen)
    # /goal: set, status, clear.
    st.rules["GOAL-CONDITION"] = [{"text": "Working on the goal."}]
    t.type("/goal GOAL-CONDITION is met", settle=False)
    t.pump(3.0)
    if c.busy(t.text()):
        t.send(ESC)
    c.idle(t, timeout=30)
    screen = c.say(t, "/goal")
    c.ok(re.search(r"GOAL-CONDITION|goal", screen, re.I), "/goal shows the goal", screen)
    screen = c.say(t, "/goal clear")
    screen = c.say(t, "/goal")
    c.ok(re.search(r"no goal|none|not set", screen, re.I), "/goal clear ends it", screen)
    # /clear while a subtask and a loop run.
    c.say(t, "/subtask SLOW-SUBTASK now")
    c.say(t, "/loop 2h LOOP-TWO look again", expect="echo:")
    screen = c.say(t, "/clear", expect="Conversation cleared")
    c.ok(t.status is None and "Conversation cleared" in screen, "/clear with a subtask and a loop running clears", screen)
    screen = c.say(t, "/tasks")
    c.ok(t.status is None, "/tasks answers after /clear", screen)
    c.ok(c.alive(t, "after-clear"), "the next prompt works after /clear", t.text())
    sent = json.dumps(st.requests[-1]["messages"])
    c.ok("seed" not in sent.replace("ping-after-clear", ""), "the new conversation does not carry the old one", sent[-500:])
    t0 = time.time()
    code = c.quit(t)
    c.ok(code == 0 and not t.left_clean() and time.time() - t0 < 12, "/exit with a subtask still running leaves cleanly, promptly",
         (code, t.left_clean(), f"{time.time() - t0:.1f}s"))


@check("E", "mode changes and prompts in quick succession never leave a turn hanging")
def mode_stress(c):
    """Seen once in a run and not again: after /plan, Shift+Tab and a prompt, 'Working… (0s)' stayed on screen.
    This repeats that sequence and its neighbours many times."""
    sb, st, t = c.session()
    c.say(t, "seed", expect="echo: seed")
    hung = []
    for i in range(25):
        t.send(ctrl("u"))
        t.type("/plan", settle=False)
        t.pump(0.05 * (i % 4))
        for _ in range(i % 3 + 1):
            t.send(kterm.BACKTAB, settle=False)
            t.pump(0.01 * (i % 5))
        t.type(f"ping-stress-{i}", settle=False)
        try:
            t.wait(f"echo: ping-stress-{i}", timeout=20)
            c.idle(t)
        except AssertionError:
            hung.append((i, t.text()[-800:]))
            t.send(ESC)
            try:
                c.idle(t)
            except AssertionError:
                break
        panel = lambda: next((l for l in reversed(t.lines()) if "127.0.0.1" in l), "")
        for _ in range(4):
            if "Default" in panel():
                break
            t.send(kterm.BACKTAB)
    c.ok(not hung, "25 rounds of /plan, Shift+Tab and a prompt: every prompt is answered", hung[:2])
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves cleanly", (t.status, t.left_clean()))
