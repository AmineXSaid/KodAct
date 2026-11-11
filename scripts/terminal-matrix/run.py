#!/usr/bin/env python3
"""The pre-production matrix for KodAct's terminal (docs/PREPRODUCTION_TEST.md).

Drives the real `kodact` binary in a pseudo-terminal against a local stub
endpoint, and checks the screen, the files and the session store after each
action. No model and no API key are needed.

    cargo build -p kodact-cli
    scripts/terminal-matrix/run.py                # the whole matrix, both displays
    scripts/terminal-matrix/run.py -k clear       # the checks whose name contains "clear"
    scripts/terminal-matrix/run.py -s B -d full   # one section, one display
    scripts/terminal-matrix/run.py -s B -x "every form"   # a section without its longest checks
    scripts/terminal-matrix/run.py --list

It writes the ledger to `target/terminal-matrix/ledger.md` (and `.json`) and
exits 1 when any row failed. Needs Python 3.9+ and `pip install pyte`.
"""

import argparse
import importlib
import json
import multiprocessing
import os
import re
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import kterm  # noqa: E402
from kterm import Sandbox, Stub, Term  # noqa: E402

CHECKS = []
SIZES = [(200, 50), (110, 36), (80, 24), (60, 20), (40, 12)]


def check(section, name, displays=("full", "inline")):
    """Register a check. It runs once per display it names."""
    def deco(fn):
        CHECKS.append((section, name, displays, fn))
        return fn
    return deco


class NotRun(Exception):
    pass


class Ctx:
    """What one check works with: sandboxes, stubs, terminals, and its ledger rows."""

    def __init__(self, section, name, display):
        self.section, self.name, self.display = section, name, display
        self.rows = []
        self._open = []

    # ledger ------------------------------------------------------------

    def ok(self, cond, what, saw=""):
        self.rows.append({"section": self.section, "check": self.name, "display": self.display, "what": what,
                          "status": "pass" if cond else "fail", "saw": "" if cond else str(saw)[-3000:]})
        return bool(cond)

    def known(self, what, why):
        """A fault seen and left: listed in the ledger with its reason; it does not fail the run."""
        self.rows.append({"section": self.section, "check": self.name, "display": self.display, "what": what,
                          "status": "known", "saw": why})

    def notrun(self, what, why):
        self.rows.append({"section": self.section, "check": self.name, "display": self.display, "what": what,
                          "status": "not run", "saw": why})

    # setup -------------------------------------------------------------

    def sandbox(self, **kw):
        sb = Sandbox(**kw)
        self._open.append(sb.close)
        return sb

    def stub(self, **kw):
        st = Stub(**kw)
        self._open.append(st.close)
        return st

    def term(self, sb, stub=None, args=(), cols=110, rows=36, cwd=None, wait=True, keyboard_protocol=False, **env):
        if self.display == "inline":
            env.setdefault("KODACT_TUI", "inline")
        t = Term(list(args), sb.env(stub, **env), cwd or sb.project, cols=cols, rows=rows, keyboard_protocol=keyboard_protocol)
        self._open.append(t.kill)
        if wait:
            t.wait("❯", timeout=15)
            t.settle()
        return t

    def session(self, cols=110, rows=36, args=(), git=True, **env):
        """The usual start: a sandbox, a stub and a terminal at the front page."""
        sb, st = self.sandbox(git=git), self.stub()
        return sb, st, self.term(sb, st, args=args, cols=cols, rows=rows, **env)

    # what a person waits for ---------------------------------------------

    @staticmethod
    def busy(text):
        return "esc stop" in text or "Type to queue" in text or "esc to interrupt" in text

    @staticmethod
    def dialog(text):
        return any(s in text for s in ("Esc to cancel", "Esc to deny", "Esc back", "Esc to close", "esc close"))

    def idle(self, t, timeout=20):
        """Wait for the turn to end. Returns the screen."""
        t.pump(0.15)
        t.wait(lambda s: not self.busy(s), timeout=timeout)
        return t.settle()

    def say(self, t, text, expect=None, timeout=20):
        """Send a prompt, wait for the turn to end, return the screen."""
        t.type(text, settle=False)
        if expect:
            t.wait(expect, timeout=timeout)
        return self.idle(t, timeout=timeout)

    def alive(self, t, tag):
        """The session still takes a prompt and answers it: what 'usable' means."""
        for _ in range(3):
            if not self.dialog(t.text()):
                break
            t.send(kterm.ESC)
        try:
            self.idle(t)
            t.send(kterm.ctrl("u"))
            t.type(f"ping-{tag}", settle=False)
            # (After an interrupted tool call the model's echo starts with KodAct's note about it.)
            t.wait(re.compile(rf"echo: .*ping-{re.escape(tag)}"), timeout=15)
            self.idle(t)
            return True
        except AssertionError:
            return False

    def quit(self, t):
        """Leave with /exit and report what the terminal was left with."""
        if t.status is None:
            for _ in range(3):
                if not self.dialog(t.text()):
                    break
                t.send(kterm.ESC)
            t.send(kterm.ctrl("u"))
            t.type("/exit", settle=False)
        return t.wait_exit(10)

    def close(self):
        for f in reversed(self._open):
            try:
                f()
            except Exception:
                pass


def run_one(job):
    section, name, display, idx = job
    fn = CHECKS[idx][3]
    c = Ctx(section, name, display)
    t0 = time.time()
    try:
        fn(c)
        if not c.rows:
            c.ok(False, "the check recorded nothing", "")
    except NotRun as e:
        c.notrun("the whole check", str(e))
    except AssertionError as e:
        c.ok(False, "the check ran to its end", str(e))
    except Exception:
        c.ok(False, "the check ran to its end", traceback.format_exc())
    finally:
        c.close()
    for r in c.rows:
        r["seconds"] = round(time.time() - t0, 1)
    return c.rows


def load():
    here = os.path.dirname(os.path.abspath(__file__))
    for f in sorted(os.listdir(os.path.join(here, "checks"))):
        if f.endswith(".py") and not f.startswith("_"):
            importlib.import_module(f"checks.{f[:-3]}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-k", help="only checks whose name contains this")
    ap.add_argument("-x", help="skip checks whose name contains this")
    ap.add_argument("-s", "--section", help="only this section (A to G)")
    ap.add_argument("-d", "--display", choices=["full", "inline"], help="only this display")
    ap.add_argument("-j", "--jobs", type=int, default=max(2, (os.cpu_count() or 2)))
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--out", default=os.path.join(kterm.ROOT, "target", "terminal-matrix"))
    ap.add_argument("-v", "--verbose", action="store_true", help="print what a failing row saw")
    a = ap.parse_args()
    load()
    jobs = []
    for i, (section, name, displays, _) in enumerate(CHECKS):
        if a.k and a.k not in name:
            continue
        if a.x and a.x in name:
            continue
        if a.section and section != a.section.upper():
            continue
        for d in displays:
            if a.display and d != a.display and d != "any":
                continue
            jobs.append((section, name, d, i))
    if a.list:
        for j in jobs:
            print(j[0], j[2].ljust(6), j[1])
        return 0
    binary = os.environ.get("KODACT_BIN") or os.path.join(kterm.ROOT, "target", "debug", "kodact")
    if not os.path.exists(binary):
        sys.exit(f"{binary} is not built: run `cargo build -p kodact-cli` first (or set KODACT_BIN)")
    rows = []
    t0 = time.time()
    with multiprocessing.Pool(a.jobs) as pool:
        for out in pool.imap_unordered(run_one, jobs):
            rows.extend(out)
            bad = [r for r in out if r["status"] == "fail"]
            head = out[0]
            print(f"{'FAIL' if bad else 'ok  '} {head['section']} {head['display']:<6} {head['check']}  "
                  f"({len(out)} rows, {head.get('seconds', 0)}s)", flush=True)
            for r in bad:
                print(f"       ✗ {r['what']}", flush=True)
                if a.verbose:
                    print("         " + r["saw"].replace("\n", "\n         "), flush=True)
    rows.sort(key=lambda r: (r["section"], r["check"], r["display"]))
    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "ledger.json"), "w") as f:
        json.dump(rows, f, indent=1)
    n = {s: sum(1 for r in rows if r["status"] == s) for s in ("pass", "fail", "known", "not run")}
    with open(os.path.join(a.out, "ledger.md"), "w") as f:
        f.write(f"# Terminal matrix ledger\n\n{n['pass']} passed, {n['fail']} failed, {n['known']} known and not fixed, {n['not run']} not run, "
                f"in {time.time() - t0:.0f}s.\n\n| | Section | Check | Display | What | Saw |\n|---|---|---|---|---|---|\n")
        for r in rows:
            mark = {"pass": "✓", "fail": "✗", "known": "! not fixed", "not run": "– not run"}[r["status"]]
            saw = r["saw"].strip().splitlines()[-1][:160].replace("|", "\\|") if r["saw"].strip() else ""
            f.write(f"| {mark} | {r['section']} | {r['check']} | {r['display']} | {r['what'].replace('|', chr(92) + '|')} | {saw} |\n")
    print(f"\n{n['pass']} passed, {n['fail']} failed, {n['known']} known and not fixed, {n['not run']} not run in {time.time() - t0:.0f}s. "
          f"Ledger: {os.path.join(a.out, 'ledger.md')}")
    return 1 if n["fail"] else 0


if __name__ == "__main__":
    # Checks import this module as `run`; keep one copy of CHECKS.
    sys.modules["run"] = sys.modules["__main__"]
    sys.exit(main())
