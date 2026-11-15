# Rules for agents working on KodAct

Before proposing or making any change to KodAct, answer: **what does it
change in real use**, for someone running KodAct on their own project, at
the terminal or in a script, not only on a benchmark? Put that answer in the
proposal. A change whose only effect is a benchmark score is not proposed.

The full rules are in KODACT.md:

@KODACT.md

## Facts about the endpoints in use

- The deep-thinking gateway never honours the reasoning level, in any form KodAct or
  `scripts/probe-effort.py` sends it (`reasoning_effort`, `reasoning.effort`,
  `chat_template_kwargs`). `--effort` changes nothing there, and `adaptiveEffort` can't work on it.
  Don't propose effort-based fixes for its long replies. The levers are the reply cap
  (`modelLimits`, 65,536 in the Harbor kit) and KodAct's own thinking time-box (`thinkingTimeBox`).
