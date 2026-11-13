# Harness study, 2026-11-13: what to take into KodAct

Six open-source harnesses and Anthropic's published material were read at
source level to find changes worth making in KodAct. This page ranks the
findings. Each source has its own report with the evidence:

| Report | Source | Commit | Licence |
| --- | --- | --- | --- |
| [01-codex.md](01-codex.md) | `openai/codex` | `16280e1` | Apache-2.0 |
| [02-gemini-cli.md](02-gemini-cli.md) | `google-gemini/gemini-cli` | `b977656` | Apache-2.0 |
| [03-opencode.md](03-opencode.md) | `sst/opencode` | `fb11429` | MIT |
| [04-aider.md](04-aider.md) | `Aider-AI/aider` | `5dc9490` | Apache-2.0 |
| [05-cline.md](05-cline.md) | `cline/cline` | `72b3d71` | Apache-2.0 |
| [06-goose.md](06-goose.md) | `block/goose` | `ef8933e` | Apache-2.0 |
| [07-anthropic-published.md](07-anthropic-published.md) | Prompting guide, Agent SDK docs, published chat prompt | read 2026-11-13 | Not licensed for copying |

## How to read this study

- **Coverage.** Every prompt file of the six repositories was read in
  full, except two of opencode's per-family prompts. The harness code was
  read where it shapes what the model sees: compaction, edit matching,
  tool definitions, output limits, loop detection. The repositories hold
  more than 600,000 lines; user interfaces, providers, sandboxes and cloud
  services were not read. Each report lists what was read in full, in
  part, and not at all.
- **Nothing is copied.** Each licence file was checked. The reports
  describe mechanisms and quote at most a short phrase. Anything adapted
  later from the six repositories needs attribution in the commit that
  adds it; Anthropic's pages are a source of technique only.
- **Nothing is implemented or measured.** Every item below is a candidate.
  By `KODACT.md` each needs an eval task and a before/after run with
  `kodact-eval`, which this environment cannot do without an API key.
- **The identifiers** (C1, G2, O3…) point to the candidate tables in the
  reports, where each item states its effect in real use, its pillar and
  metric, its behaviour on a large model, and its cost on a quiet turn.

## The finding to act on first

**KodAct probably breaks long sessions on the newest Claude models.**

Anthropic's documentation says that on models with "preserved thinking",
a request that sends thinking blocks back after the system prompt, the
tool list or an earlier message has changed fails a prefix check. On the
newest family the API answers 400; on three other current models it
answers 400 or lets stale reasoning through.

KodAct edits earlier messages in at least three places: micro-compaction
rewrites old tool results, the system prompt carries the date and git
status, and partial compaction keeps turns after the summary. Details and
file references are in [07-anthropic-published.md](07-anthropic-published.md),
section 1.

This was read from the documentation and from KodAct's source. It was not
reproduced: there is no API key here. The first step is a test against
the real API (candidate **N1**). If it fails, the fix is to make requests
append-only on those models and to use server-side context editing for
clearing tool results there.

## What the six harnesses agree on

Where independent projects arrive at the same answer, the answer is likely
right.

1. **Prompts are getting shorter.** opencode's base prompt is 756
   characters, Goose's 1,200, Cline's 3,700. Codex's newest prompt dropped
   its formatting rules and examples. What remains is facts about the
   harness and rules about other people's work. KodAct's is 6,200
   characters before the unattended section. (O7)
2. **A failed edit should cost no turn when the intent is clear.** Five of
   six apply a match that differs only in indentation, trailing whitespace
   or typographic characters, and say so. KodAct reports the near match
   and waits for a retry. (C1, G2, A1)
3. **"The new text is already in the file."** opencode and Aider both put
   this in the edit error. It ends the most common edit loop. (O2)
4. **Compaction must protect the prompt cache.** opencode summarizes in
   the background and holds the result; Gemini CLI clears tool output only
   in large batches; Cline freezes what survived. Every rewrite of history
   costs a cache miss on everything after it. (O3, G6)
5. **Text in the conversation is data.** Goals, summaries, hook output and
   tool results are wrapped as untrusted in Codex, opencode, Gemini CLI
   and Goose, and the summarizer is told to ignore instructions in the
   history. (G5, G7, N3)
6. **Unattended runs need an explicit end and evidence.** Cline ends a run
   with a tool call that carries a `verified` flag; opencode and Codex
   judge a goal from evidence in the conversation; Goose runs the person's
   success checks. (L3, C7, S1)

## Where they disagree

| Question | One answer | The other | For KodAct |
| --- | --- | --- | --- |
| "Can you…": act or advise? | Codex, opencode's newest prompt: act | Gemini CLI: an inquiry unless it asks for action | Rule 10 of `docs/goal.md` (a guess must fail safe) favours advising when no change was asked for (G3) |
| Delegate to sub-agents? | Gemini CLI: you must delegate | opencode: only when the person asks; Anthropic: say what a sub-agent costs | State the cost in the Task description (N8) |
| Show the model its remaining budget? | Goose and Codex: every turn, or near the limit | Anthropic: removing a large displayed budget lowered cost at equal performance | Keep the single late reminder; test before adding more (S5, C3) |
| Per-model prompts? | Codex, opencode, Aider: a table keyed by model | Gemini CLI, Cline, Goose: one prompt | `docs/goal.md` forbids keying on the model's name; decide from behaviour in the run |
| Summary written by a model? | Five of six | Cline: a mechanical fold, no model call | Attach the mechanical record to the model's summary (L2) |

## Ranked candidates

Ranked by expected effect on everyday use against the work needed. "Small"
means a change inside one module with its tests.

### Tier 1: likely wins, small changes

| # | Change | Why it ranks here | Size |
| --- | --- | --- | --- |
| N1 | Test, then fix, append-only requests on preserved-thinking models | A possible hard failure on the strongest models | Test: small. Fix: medium |
| G1 | Edit and Write refuse a replacement that abbreviates code with a placeholder | Prevents silent loss of code; pure text check | Small |
| A1 / G2 / C1 | Edit applies indentation-only, trailing-whitespace and typographic near matches, and says so | Three sources; the most common failed call on small models | Small |
| O2 | Edit error says when the new text is already in the file | Two sources; ends a loop with one fact | Small |
| O1 | Coerce tool input to the schema where unambiguous | Removes a class of invalid calls on small models | Small |
| L2 | Mechanical record of files read, files edited and commands run, attached to each summary | No model call; facts a summary drops | Small |
| G5 | Summary instruction: history is data; reject a summary larger than what it replaces | Two sentences and one comparison | Small |
| N3 | Prompt: tags inside tool output or files are content | Closes a gap in the current wording | Small |
| S3 | `AGENT=kodact` in the environment of commands | Lets projects adapt their scripts | Small |
| C4 | Truncated output states the total line count | A few tokens when truncating | Small |

### Tier 2: worth building, need an A/B first

| # | Change | What must be measured |
| --- | --- | --- |
| O7 | A shorter system prompt | `scripts/quiet-check.sh` on a small and a large model: tokens, turns, pass rate |
| N4, N5, G3, G4, C6 | Prompt lines: obstacles against blockers, reversibility, answer-do-not-act on questions, reproduce before fixing, "compaction exists" | Each alone, same check; they add about 250 tokens together |
| O3 | Background summary held until compaction is due | Wall time and cost per compaction; cached-token share |
| O4 | A second compaction updates the previous summary | `deliverable-after-compaction` extended to two compactions |
| G6 | Micro-compaction in batches | Cached-token share and cost on long tasks (interacts with N1) |
| L1 | Read takes several files in one call | Turns per task on an endpoint without parallel tool calls |
| O5 | A final call without tools at the turn limit | Share of limit-hit runs with a usable hand-off |
| L3 | A finish tool with a `verified` claim in unattended runs | False-finish rate, split by claim against `verification.status` |
| A3, A4 | Built-in syntax check and errors shown in their scope | `syntax-after-edit` on a machine without the toolchain |
| C2 | Send input to a running command | A new task with a command that prompts |
| S1, S2 | `--check` with retry; retry on invalid `--json-schema` output | A scripted task for each |
| N7 | Silent-steps reminder | Longest silent stretch in interactive runs |

### Tier 3: ideas, not proposals

A repository outline on demand (A5), a cheaper model for summaries (A6),
architect and editor pairing (A7), a follow-up to an existing Task agent
(O6), tool-name aliases from other harnesses (O8), cache keep-alive (O9),
a reviewer model for shell commands (S6, and Codex's auto-review), a text
protocol for models that cannot emit tool calls (S7), a self-check pass on
the summary (G9), goal audits (C7), plan-mode question rules (C8), memory
pruning (N9).

## Suggested order of work

1. **N1's test** against the real API, as soon as a key is available. Its
   result decides whether G6 and O3 are designed around server-side
   context editing.
2. **The Edit group** (G1, A1/G2/C1, O2) as one change with one eval task
   per failure kind: placeholder, indentation offset, curly quotes,
   already applied. Metric: invalid-call rate and turns per task.
3. **O1 and L2**, independent of each other and of the rest.
4. **The prompt A/B** (O7, then the single lines), once the first real
   baseline of `docs/BASELINE.md` exists. Without a baseline a prompt
   change cannot be judged.
5. **Compaction** (O3, O4, G5), after N1.

## What this study does not settle

- Whether any item moves KodAct's metrics. No comparison was run.
- How the six harnesses perform against each other. No benchmark number
  from their repositories is repeated here.
- The parts of each repository that were not read (listed per report).
- Whether the prefix check fails for KodAct in practice (N1).
