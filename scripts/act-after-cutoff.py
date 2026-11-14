#!/usr/bin/env python3
"""After KodAct hands a model back its reasoning, does the next reply act?

A reply that was all reasoning (cut off at the output limit, stopped by the thinking time
limit, or ended with no answer) gets an act reminder. Where the provider doesn't send
reasoning back, the reminder carries that reasoning, and when part of it doesn't fit, the
whole of it is saved in the session's spill directory. This reads stream-json transcripts
(`kodact -p --output-format stream-json`: a Harbor run's kodact.jsonl, a kodact-eval run's
transcript.jsonl) and reports, for each handback:

  - what the next reply did: called a tool ("acted"), answered in text ("answered"), reasoned to
    another reminder or to the limit again ("again"), or the turn ended ("ended");
  - whether a later tool call read the saved file;
  - how many characters of reasoning were handed back (0 where the provider sends reasoning
    back itself: those are counted apart, as reminders without a handback).

Needs no package. Usage:

  scripts/act-after-cutoff.py PATH... [--json]

Each PATH is a transcript, or a directory searched for kodact.jsonl and transcript.jsonl.
"""

import argparse
import json
import os
import sys

NAMES = ("kodact.jsonl", "transcript.jsonl")


def transcripts(paths):
    for p in paths:
        if os.path.isdir(p):
            for root, _, files in os.walk(p):
                for f in sorted(files):
                    if f in NAMES:
                        yield os.path.join(root, f)
        else:
            yield p


def events(path):
    out = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line.startswith("{"):
                try:
                    out.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return out


KINDS = ("reasoning_cutoff", "empty_reply")


def handback(ev):
    """The kind of reminder this event is, when it is one for a reply that was all reasoning."""
    if ev.get("type") != "system":
        return None
    if ev.get("subtype") == "act_reminder" and ev.get("kind") in KINDS:
        return ev["kind"]
    if ev.get("subtype") == "nudge" and ev.get("kind") == "thinking_time_box":
        return "thinking_time_box"
    return None


def top_level(ev):
    return ev.get("type") == "assistant" and not ev.get("parent_tool_use_id")


def blocks(ev, kind):
    content = (ev.get("message") or {}).get("content") or []
    return [b for b in content if isinstance(b, dict) and b.get("type") == kind]


def calls(ev):
    """The tool calls of a top-level assistant event."""
    return blocks(ev, "tool_use") if top_level(ev) else []


def after(evs, i):
    """What the next reply did after the reminder at `i`, and the first tool call it made."""
    for ev in evs[i + 1 :]:
        if handback(ev):
            return "again", None
        c = calls(ev)
        if c:
            return "acted", c[0].get("name")
        if top_level(ev):
            if any(str(b.get("text") or "").strip() for b in blocks(ev, "text")):
                return "answered", None
            # Past the last reminder in a row, a reply that reasons to the limit gets none.
            if (ev.get("message") or {}).get("stop_reason") == "max_tokens":
                return "again", None
        if ev.get("type") == "result":
            return "ended", None
    return "ended", None


def report(path):
    evs = events(path)
    rows = []
    for i, ev in enumerate(evs):
        kind = handback(ev)
        if not kind:
            continue
        outcome, first_call = after(evs, i)
        saved = ev.get("saved")
        read = None
        if saved:
            # Read at any point later in the run, by any tool (Read, Grep, a shell command).
            name = os.path.basename(saved)
            later = [json.dumps(c.get("input")) for e in evs[i + 1 :] for c in calls(e)]
            read = any(saved in text or name in text for text in later)
        rows.append(
            {
                "kind": kind,
                "reasoning_chars": ev.get("reasoning_chars") or 0,
                "saved": saved,
                "next": outcome,
                "first_call": first_call,
                "read_saved": read,
            }
        )
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--json", action="store_true", help="print one JSON object per transcript")
    args = ap.parse_args()
    total = {"handbacks": 0, "plain": 0, "acted": 0, "answered": 0, "again": 0, "ended": 0, "saved": 0, "read_saved": 0}
    found = False
    for path in transcripts(args.paths):
        found = True
        rows = report(path)
        for r in rows:
            if not r["reasoning_chars"]:
                total["plain"] += 1
                continue
            total["handbacks"] += 1
            total[r["next"]] += 1
            total["saved"] += bool(r["saved"])
            total["read_saved"] += bool(r["read_saved"])
        if args.json:
            print(json.dumps({"transcript": path, "handbacks": rows}))
            continue
        if not rows:
            continue
        print(path)
        for n, r in enumerate(rows, 1):
            what = {
                "acted": f"acted ({r['first_call']})",
                "answered": "answered",
                "again": "reasoned to another reminder or the limit again",
            }.get(r["next"], "the turn ended")
            saved = ""
            if r["saved"]:
                saved = f" · saved {os.path.basename(r['saved'])}, read: {'yes' if r['read_saved'] else 'no'}"
            chars = f"{r['reasoning_chars']:,} characters handed back" if r["reasoning_chars"] else "nothing handed back"
            print(f"  #{n} {r['kind']}: {chars}{saved} → {what}")
    if not found:
        sys.exit("no transcript found")
    if not args.json:
        print(
            f"{total['handbacks']} handbacks: {total['acted']} acted next, {total['answered']} answered, "
            f"{total['again']} reasoned again, {total['ended']} ended the turn; {total['read_saved']} of "
            f"{total['saved']} saved files read. {total['plain']} reminders handed nothing back."
        )


if __name__ == "__main__":
    main()
