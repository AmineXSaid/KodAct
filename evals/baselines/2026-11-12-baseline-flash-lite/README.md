# First recorded baseline (2026-11-12)

KodAct at `3b0f632` with main merged (`401f564`), model `gemini-3.1-flash-lite` through its
OpenAI-compatible endpoint (free tier), shell commands in the OS sandbox
(`--sandbox workspace-write --permission-mode acceptEdits --allowedTools Bash`), every nudge
off, one run per task over the verification, recovery and tool-design pillars: 25 tasks.

Pass rate 88%, false-finish 4%, 11.4 turns and about 132k tokens per task, 12% of tool calls
errored. Costs read $0: KodAct has no price for this model. One run per task: a task either
way is noise; it is the reference the next A/B starts from, not a verdict on any task.

`report.md` and `report.json` are `kodact-eval`'s reports, without the workspaces' paths and
the checks' output.
