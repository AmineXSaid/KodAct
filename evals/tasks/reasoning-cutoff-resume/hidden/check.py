"""scene.json holds the twelve values; and when a reply was cut off while it was all reasoning,
the reply after the reminder acted (called a tool or answered) instead of reasoning to the limit
again.

task.json caps the reply at 4,000 tokens, so a model that works the values out in its reasoning
is cut off mid-analysis, and sets a 20,000-token window, so the reasoning doesn't all fit the
room left before compaction: part comes back and the whole is saved to a file. The window setting
applies to a model whose own window isn't known (KODACT_CONTEXT_WINDOW); with a known one, the
reasoning comes back whole. kodact-eval writes the run's stream next to the workspace
(transcript.jsonl); without one (task validation), or with no cut-off in it, only scene.json is
checked, and the output says so.
"""

import json
import os
import struct
import sys

HEX = (
    "00 00 40 3f 00 00 00 bf 00 00 88 c0 00 00 90 3f 2e f9 e4 3e 2e f9 64 3f "
    "cd cc 4c be 00 00 c0 bf 66 66 66 3f 33 33 b3 3e ec 51 38 3e cd cc 0c 40"
)
KEYS = [
    "centre_x", "centre_y", "centre_z", "radius", "light_x", "light_y", "light_z",
    "floor_y", "colour_a", "colour_b", "ambient", "gamma",
]
want = dict(zip(KEYS, struct.unpack("<12f", bytes.fromhex(HEX.replace(" ", "")))))

if not os.path.exists("scene.json"):
    sys.exit("scene.json is missing")
try:
    got = json.load(open("scene.json"))
except ValueError as e:
    sys.exit(f"scene.json is not JSON: {e}")
if not isinstance(got, dict):
    sys.exit("scene.json is not one JSON object")
for key, value in want.items():
    g = got.get(key)
    if isinstance(g, bool) or not isinstance(g, (int, float)) or abs(g - value) > 1e-4:
        sys.exit(f"{key}: expected {round(value, 4)}, found {g!r}")


def acted(ev):
    """A reply of the main agent that called a tool or answered in text."""
    if ev.get("type") != "assistant" or ev.get("parent_tool_use_id"):
        return False
    content = (ev.get("message") or {}).get("content") or []
    return any(
        b.get("type") == "tool_use" or (b.get("type") == "text" and str(b.get("text") or "").strip())
        for b in content
    )


def again(ev):
    """Another reminder for a reply that was all reasoning, or such a reply cut off with no reminder."""
    if ev.get("type") == "system":
        return (ev.get("subtype") == "act_reminder" and ev.get("kind") in ("reasoning_cutoff", "empty_reply")) or (
            ev.get("subtype") == "nudge" and ev.get("kind") == "thinking_time_box"
        )
    msg = ev.get("message") or {}
    return ev.get("type") == "assistant" and not ev.get("parent_tool_use_id") and msg.get("stop_reason") == "max_tokens"


stream = os.path.join(os.path.dirname(os.getcwd()), "transcript.jsonl")
if not os.path.exists(stream):
    sys.exit(0)
events = []
for line in open(stream, encoding="utf-8", errors="replace"):
    try:
        events.append(json.loads(line))
    except ValueError:
        pass  # a line cut short when the run timed out
first = next(
    (i for i, ev in enumerate(events) if ev.get("subtype") == "act_reminder" and ev.get("kind") == "reasoning_cutoff"),
    None,
)
if first is None:
    print("no reply was cut off while reasoning: only scene.json was checked")
    sys.exit(0)
print(f"cut off while reasoning; {events[first].get('reasoning_chars', 0):,} characters handed back")
for ev in events[first + 1 :]:
    if acted(ev):
        break
    if again(ev) or ev.get("type") == "result":
        sys.exit("after its reasoning was handed back, the next reply did not act: it reasoned to the limit again")
