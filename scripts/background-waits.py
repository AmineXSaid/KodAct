#!/usr/bin/env python3
"""How KodAct's end-of-turn waits for background commands went, across recorded runs.

A turn that would end while a command moved to the background at its Bash timeout still runs
waits for it: up to 10 minutes, and a minute short of `--max-time` (event `background_wait`).
On 2026-11-15, largest-eigenval waited 80 s for a symbol scan the model had dropped 8 minutes
earlier, with about 140 s of its run left, then killed it.

One run is a lead, not a reason (docs/goal.md, rule 4). This counts every wait in the runs given,
to decide whether a wait should keep out of the run's last fifth. For each wait it prints:
- the time limit, and the time left when the wait began;
- whether the wait began inside the run's last fifth (the window of the wrap-up warning);
- how it ended: the command finished, or still ran when the wait was over;
- the model's next tool calls.

    scripts/background-waits.py jobs/            # Harbor job folders: <trial>/agent/kodact.jsonl
    scripts/background-waits.py evals/results/   # kodact-eval runs: transcript.jsonl

Reads a stream's `background_wait` events and what follows them. The time left is known only for
a wait the time limit shortened (`limit_ms` under 600,000: KodAct waited until a minute before the
limit). The limit itself comes from the trial's run.json (`--max-time`) when there is one.
"""

import json
import sys
from pathlib import Path

FULL_WAIT_MS = 600_000  # BACKGROUND_WAIT in kodact-engine's engine.rs
RESERVE_S = 60  # the minute a wait leaves before --max-time


def streams(roots: list[Path]):
    for root in roots:
        for name in ("kodact.jsonl", "transcript.jsonl"):
            yield from sorted(root.rglob(name))


def time_limit(stream: Path) -> float | None:
    """The run's --max-time in seconds, from the trial's run.json (Harbor) when there is one."""
    try:
        flags = json.loads((stream.parent / "run.json").read_text()).get("flags") or []
    except (OSError, ValueError):
        return None
    for flag, value in zip(flags, flags[1:]):
        if flag == "--max-time":
            try:
                return float(value)
            except ValueError:
                return None
    return None


def events(stream: Path) -> list[dict]:
    out = []
    for line in stream.read_text(errors="replace").splitlines():
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        if isinstance(ev, dict):
            out.append(ev)
    return out


def user_text(ev: dict) -> str:
    content = (ev.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    return " ".join(b.get("text", "") for b in content or [] if isinstance(b, dict) and b.get("type") == "text")


def main() -> None:
    roots = [Path(a) for a in sys.argv[1:]] or [Path("jobs")]
    rows = []
    for stream in streams(roots):
        evs = events(stream)
        limit = time_limit(stream)
        for i, ev in enumerate(evs):
            if ev.get("type") != "system" or ev.get("subtype") != "background_wait":
                continue
            limit_ms = int(ev.get("limit_ms") or 0)
            left = limit_ms / 1000 + RESERVE_S if limit_ms < FULL_WAIT_MS else None
            last_fifth = None
            if left is not None and limit:
                last_fifth = left <= max(limit / 5, 60)
            ended, calls = "?", []
            for later in evs[i + 1 :]:
                if later.get("type") == "system" and later.get("subtype") == "background_exit":
                    ended = "finished"
                elif later.get("type") == "user" and "still running" in user_text(later):
                    ended = "still running"
                elif later.get("type") == "assistant":
                    content = (later.get("message") or {}).get("content") or []
                    calls = [b.get("name") for b in content if b.get("type") == "tool_use"] or ["(answer)"]
                    break
            rows.append((stream, limit, left, last_fifth, limit_ms, ended, calls))

    for stream, limit, left, last_fifth, limit_ms, ended, calls in rows:
        where = stream.parent.parent.name if stream.name == "kodact.jsonl" else stream.parent.name
        left_s = f"{left:.0f}s left" if left is not None else "over 10m left"
        limit_s = f"of {limit:.0f}s" if limit else "no limit known"
        fifth = {True: "in the last fifth", False: "before the last fifth", None: ""}[last_fifth]
        print(f"{where}: waited up to {limit_ms / 1000:.0f}s, {left_s} {limit_s} {fifth}; "
              f"{ended}; next: {', '.join(calls) or '(none)'}")
    late = [r for r in rows if r[3]]
    print(f"\n{len(rows)} waits; {len(late)} began in the run's last fifth, of which "
          f"{sum(1 for r in late if r[5] == 'still running')} ended with the command still running and "
          f"{sum(1 for r in late if 'KillShell' in r[6])} were followed by KillShell.")


if __name__ == "__main__":
    main()
