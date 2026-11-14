#!/usr/bin/env python3
"""After KodAct hands a model back its reasoning, does the next reply act?

A reply that was all reasoning (cut off at the output limit, stopped by the thinking time
limit, or ended with no answer) gets an act reminder. Where the provider doesn't send
reasoning back, the reminder carries that reasoning, and when part of it doesn't fit, the
whole of it is saved in the session's spill directory. This reads stream-json transcripts
(`kodact -p --output-format stream-json`: a Harbor run's kodact.jsonl, a kodact-eval run's
transcript.jsonl) and reports, for each handback:

  - what the next reply did: called a tool ("acted"), reasoned to another reminder ("again"),
    or ended the turn ("ended");
  - whether a later tool call read the saved file;
  - how many characters of reasoning were handed back.

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


def handback(ev):
    """The kind of handback this event is, or None."""
    if ev.get("type") != "system":
        return None
    if ev.get("subtype") == "act_reminder":
        return ev.get("kind") or "act_reminder"
    if ev.get("subtype") == "nudge" and ev.get("kind") == "thinking_time_box":
        return "thinking_time_box"
    return None


def calls(ev):
    """The tool calls of a top-level assistant event."""
    if ev.get("type") != "assistant" or ev.get("parent_tool_use_id"):
        return []
    content = (ev.get("message") or {}).get("content") or []
    return [b for b in content if isinstance(b, dict) and b.get("type") == "tool_use"]


def after(evs, i):
    """What the next reply did after the handback at `i`, and the first tool call it made."""
    for ev in evs[i + 1 :]:
        if handback(ev):
            return "again", None
        c = calls(ev)
        if c:
            return "acted", c[0].get("name")
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
    total = {"handbacks": 0, "acted": 0, "again": 0, "ended": 0, "saved": 0, "read_saved": 0}
    found = False
    for path in transcripts(args.paths):
        found = True
        rows = report(path)
        for r in rows:
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
            what = {"acted": f"acted ({r['first_call']})", "again": "no action before the next reminder"}.get(
                r["next"], "the turn ended"
            )
            saved = ""
            if r["saved"]:
                saved = f" · saved {os.path.basename(r['saved'])}, read: {'yes' if r['read_saved'] else 'no'}"
            print(f"  #{n} {r['kind']}: {r['reasoning_chars']:,} characters handed back{saved} → {what}")
    if not found:
        sys.exit("no transcript found")
    if not args.json:
        print(
            f"{total['handbacks']} handbacks: {total['acted']} acted next, {total['again']} reached another "
            f"reminder first, {total['ended']} ended the turn; {total['read_saved']} of {total['saved']} saved "
            "files read"
        )


if __name__ == "__main__":
    main()
