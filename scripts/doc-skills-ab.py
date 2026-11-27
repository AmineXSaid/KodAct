#!/usr/bin/env python3
"""Document skills, A against B: the doc-* eval tasks run by one agent with
two sets of document skills, and scored by the tasks' own checks.

    scripts/doc-skills-ab.py --a NAME=DIR --b NAME=DIR [--agent claude|kodact]
                             [--model M] [--repeat N] [--jobs J] [--only TASK]

Each DIR holds one folder per document skill (pptx, docx, xlsx, pdf, each with
its SKILL.md). Each run gets a fresh copy of the task's files with that arm's
skills in `.claude/skills/`, and the same prompt in both arms: the task's
instruction and one line saying where the skills are. The agent's own skill
loading is switched off, so it can only use the arm's skills (a host that
injects its own plugins can't leak them in). `${KODACT_SKILL_DIR}`,
`${CLAUDE_SKILL_DIR}` and `${CLAUDE_PLUGIN_ROOT}`-style paths in the skills are
written out as the copy's real paths, the same for both arms.

Results go to evals/results/doc-ab-<stamp>/: per run, the agent's JSON output,
the check's verdict, the files it made, and a PDF render of each document; and
summary.md, the table.
"""

import argparse
import concurrent.futures as cf
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TASKS = os.path.join(ROOT, "evals", "tasks")
KINDS = ("pptx", "docx", "xlsx", "pdf")
HINT = (
    "\n\nDocument skills for this are in .claude/skills/ (one folder per file type: "
    "pptx, docx, xlsx, pdf). Read the matching SKILL.md before you start and follow it."
)


def arm(spec):
    name, _, path = spec.partition("=")
    if not path or not os.path.isdir(path):
        sys.exit(f"--a/--b want NAME=DIR with a skills folder, got {spec!r}")
    return name, os.path.abspath(path)


def install(skills, ws):
    """Copy an arm's skills into the workspace, their own-folder paths written out."""
    dest = os.path.join(ws, ".claude", "skills")
    os.makedirs(dest)
    for kind in KINDS:
        if os.path.isdir(os.path.join(skills, kind)):
            shutil.copytree(os.path.join(skills, kind), os.path.join(dest, kind))
    # Files a skill reaches above its folder (a plugin's shared scripts).
    shared = os.path.join(os.path.dirname(skills), "scripts")
    if os.path.isdir(shared):
        shutil.copytree(shared, os.path.join(ws, ".claude", "scripts"))
    root = os.path.join(ws, ".claude")
    for kind in KINDS:
        d = os.path.join(dest, kind)
        for base, _, files in os.walk(d):
            for f in files:
                if not f.endswith((".md", ".txt")):
                    continue
                p = os.path.join(base, f)
                s = open(p, encoding="utf8").read()
                t = re.sub(r"\$\{(KODACT|CLAUDE)_SKILL_DIR\}", d, s)
                t = re.sub(r"\$\{(KODACT|CLAUDE)_PLUGIN_ROOT\}", root, t)
                if t != s:
                    open(p, "w", encoding="utf8").write(t)


def command(agent, model, prompt, cfg):
    if agent == "claude":
        cmd = ["claude", "-p", prompt, "--output-format", "json", "--disallowedTools", "Skill",
               "--allowedTools", "Bash Read Write Edit Glob Grep"]
        env = {"CLAUDE_CONFIG_DIR": cfg}
    else:
        cmd = ["kodact", "-p", prompt, "--output-format", "json", "--disallowedTools", "Skill",
               "--dangerously-skip-permissions"]
        env = {"KODACT_CONFIG_DIR": cfg}
    if model:
        cmd += ["--model", model]
    return cmd, env


def render(path, out):
    tmp = tempfile.mkdtemp()
    subprocess.run(["soffice", f"-env:UserInstallation=file://{tmp}/p", "--headless", "--convert-to", "pdf",
                    "--outdir", out, path], capture_output=True, timeout=300)
    shutil.rmtree(tmp, ignore_errors=True)


def run(task, arm_name, skills, n, args, out):
    spec = json.load(open(os.path.join(TASKS, task, "task.json")))
    tag = f"{task}--{arm_name}--{n}"
    home = tempfile.mkdtemp(prefix="docab-")
    ws = os.path.join(home, "ws")
    shutil.copytree(os.path.join(TASKS, task, "repo"), ws)
    install(skills, ws)
    before = set(os.listdir(ws))
    cfg = os.path.join(home, "cfg")
    os.makedirs(cfg)
    cmd, extra = command(args.agent, args.model, spec["instruction"] + HINT, cfg)
    # Its own HOME: LibreOffice runs started by parallel agents don't share a profile.
    env = {**os.environ, **extra, "HOME": home}
    t0 = time.time()
    try:
        p = subprocess.run(cmd, cwd=ws, env=env, capture_output=True, text=True,
                           timeout=spec.get("timeout_secs", 1200))
        raw, timed_out = p.stdout, False
    except subprocess.TimeoutExpired as e:
        raw, timed_out = (e.stdout or b"").decode() if isinstance(e.stdout, bytes) else (e.stdout or ""), True
    secs = time.time() - t0
    try:
        result = json.loads(raw.strip().splitlines()[-1]) if raw.strip() else {}
    except (json.JSONDecodeError, IndexError):
        result = {}
    check = subprocess.run(spec["check"], shell=True, cwd=ws, capture_output=True, text=True, timeout=600,
                           env={**os.environ, "KODACT_EVAL_TASK_DIR": os.path.join(TASKS, task)})
    verdict = (check.stdout + check.stderr).strip().splitlines()[-1:] or [""]
    keep = os.path.join(out, tag)
    os.makedirs(keep)
    for f in sorted(set(os.listdir(ws)) - before):
        src = os.path.join(ws, f)
        if os.path.isfile(src) and os.path.getsize(src) < 20_000_000:
            shutil.copy(src, keep)
            if f.endswith((".pptx", ".docx")):
                render(os.path.join(keep, f), keep)
    json.dump(result, open(os.path.join(keep, "agent.json"), "w"), indent=1)
    shutil.rmtree(home, ignore_errors=True)
    return {
        "task": task, "arm": arm_name, "n": n, "pass": check.returncode == 0, "why": verdict[0],
        "secs": round(secs), "timed_out": timed_out, "turns": result.get("num_turns"),
        "cost": result.get("total_cost_usd"),
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--agent", default="claude", choices=["claude", "kodact"])
    ap.add_argument("--model", default="")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--only", action="append", default=[])
    args = ap.parse_args()
    arms = [arm(args.a), arm(args.b)]
    tasks = sorted(t for t in os.listdir(TASKS) if t.startswith("doc-") and os.path.isdir(os.path.join(TASKS, t))
                   and (not args.only or any(o in t for o in args.only)))
    out = os.path.join(ROOT, "evals", "results", time.strftime("doc-ab-%Y%m%d-%H%M%S"))
    os.makedirs(out)
    jobs = [(t, name, d, n) for n in range(1, args.repeat + 1) for t in tasks for name, d in arms]
    rows = []
    with cf.ThreadPoolExecutor(args.jobs) as ex:
        for f in cf.as_completed([ex.submit(run, *j, args, out) for j in jobs]):
            r = f.result()
            rows.append(r)
            print(f"{'PASS' if r['pass'] else 'fail'}  {r['task']:<22} {r['arm']:<10} {r['secs']:>5}s  {r['why'][:90]}", flush=True)
    json.dump(rows, open(os.path.join(out, "runs.json"), "w"), indent=1)
    lines = [f"# Document skills: {arms[0][0]} vs {arms[1][0]}", "",
             f"Agent `{args.agent}`, model `{args.model or 'default'}`, {args.repeat} run(s) per task and arm.", "",
             f"| Task | {arms[0][0]} | {arms[1][0]} |", "| --- | --- | --- |"]
    for t in tasks:
        cells = []
        for name, _ in arms:
            rs = [r for r in rows if r["task"] == t and r["arm"] == name]
            ok = sum(r["pass"] for r in rs)
            secs = sum(r["secs"] for r in rs) / max(1, len(rs))
            cost = [r["cost"] for r in rs if r["cost"] is not None]
            why = "; ".join(r["why"] for r in rs if not r["pass"])[:120]
            cell = f"{ok}/{len(rs)} · {secs:.0f}s" + (f" · ${sum(cost) / len(cost):.2f}" if cost else "")
            cells.append(cell + (f" · {why}" if why else ""))
        lines.append(f"| {t} | {cells[0]} | {cells[1]} |")
    for name, _ in arms:
        rs = [r for r in rows if r["arm"] == name]
        lines.append("")
        lines.append(f"**{name}**: {sum(r['pass'] for r in rs)}/{len(rs)} passed, "
                     f"{sum(r['secs'] for r in rs) / max(1, len(rs)):.0f}s a run on average.")
    open(os.path.join(out, "summary.md"), "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nresults: {out}")


if __name__ == "__main__":
    main()
