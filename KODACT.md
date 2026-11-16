# Working on KodAct

## The rule for every feature

Read `docs/GOALS.md` before implementing anything. KodAct's goal is the best
measured agent harness per model, per dollar and per minute. Matching the
reference CLI is the baseline, not the goal.

For each feature or harness change:

1. **Say what it changes in real use**, before proposing it: for someone
   running KodAct on their own project, at the terminal or in a script, not
   only on a benchmark. A change whose only effect is a benchmark score is not
   proposed.
2. **Check it against "Fix, don't fit"** in `docs/goal.md`, for every fix:
   limits come from the model, never from the endpoint that showed the problem;
   say how the fix behaves on a large model with a big context window; fix the
   mechanism, not the task.
3. **Name the pillar** it serves (context management, failure recovery,
   verification, tool design, long-horizon execution, or compatibility) and the
   metric it should move.
4. **Add or update an eval task** in `evals/tasks/` that exercises it.
5. **Measure it.** Run `kodact-eval` before and after (`kodact-eval run`, then
   `kodact-eval compare`), or, where no API key is available, state that the
   comparison is still owed.
6. **Write the pillar in the commit message:** `Goals: <pillar> / <metric>`.

Compatibility work also cites its `docs/PARITY.md` row and test.

## Gates before every push

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets   # no warnings
cargo test --workspace
```

## Naming

KodAct uses only KodAct names: `KODACT_*` environment variables, `.kodact/` and
`KODACT.md` for configuration, and neutral wording in docs. API wire constants
stay confined to `crates/kodact-api/src/messages.rs`.

One exception, for compatibility: skills, commands, agents and plugins written
for the reference CLI load unchanged. So KodAct also reads its folders
(`~/.claude/<kind>`, or `$CLAUDE_CONFIG_DIR`, and `.claude/<kind>` from the
repository root down), its plugin manifest (`.claude-plugin/plugin.json`) and
its `${CLAUDE_*}` placeholders, each as an alias of the KodAct name
(`${KODACT_SKILL_DIR}`, `${KODACT_PLUGIN_ROOT}`…). KodAct's own files and docs
keep the KodAct names.

## Facts about the endpoints in use

- The deep-thinking gateway never honours the reasoning level, in any form KodAct or
  `scripts/probe-effort.py` sends it (`reasoning_effort`, `reasoning.effort`,
  `chat_template_kwargs`). `--effort` changes nothing there, and `adaptiveEffort` can't work on it.
  Don't propose effort-based fixes for its long replies. The levers are the reply cap
  (`modelLimits`, 65,536 in the Harbor kit) and KodAct's own thinking time-box (`thinkingTimeBox`).
