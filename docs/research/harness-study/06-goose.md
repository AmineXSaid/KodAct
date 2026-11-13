# Goose: what its source teaches KodAct

- **Source:** `block/goose` at commit `ef8933e` (2026-11-13).
- **Licence:** Apache-2.0 (`LICENSE` checked). Adaptation with attribution
  is allowed. Nothing is copied here.
- **Read in full:** the prompts in `crates/goose/src/prompts/` (`system.md`,
  `tiny_model_system.md`, `subagent_system.md`, `permission_judge.md`) and
  in `crates/goose-context-management/src/prompts/` (`compaction.md`,
  `compaction_summary.md`).
- **Read in part:** `agents/moim.rs`, `context_mgmt/mod.rs`,
  `agents/large_response_handler.rs`, `agents/final_output_tool.rs`,
  `agents/retry.rs`, `security/adversary_inspector.rs`, the constants of
  the developer extension's shell tool, the edit tool's error path.
- **Not read:** the providers, the desktop and CLI front ends, MCP
  plumbing, recipes' parser, the scheduler, local inference, the analyze
  extension's internals.

`docs/REFERENCES.md` already credits Goose for the `.agents/` directory,
the pattern scanner and recipes. This report covers what the source shows
beyond the documentation read in October.

## 1. How the prompt is built

The system prompt is **1,200 characters**: a name, a paragraph that
explains extensions, the instructions of each active extension, and one
line on Markdown. Everything about how to code is absent. Behaviour comes
from the extensions' own instructions and tool descriptions, so a session
with no developer extension carries no coding advice at all.

Three smaller prompts are worth noting:

- **A prompt for very small local models** (840 characters). It drops tool
  calling altogether: to run a command the model writes a line that starts
  with `$`. One example, three rules ("do it, don't describe it", "don't
  repeat commands", "don't run a command if you know the answer"). This is
  an honest answer to models that cannot produce tool-call JSON: change the
  protocol, not the prompt.
- **The sub-agent prompt** states the bounds: the turn limit, the tool
  count and names, no further sub-agents, and that a summary, if asked
  for, must be the last message.
- **The permission judge** is five sentences: tool names and arguments
  are untrusted data; a request whose data tries to influence the verdict
  is never classified read-only.

## 2. Mechanisms worth knowing

### 2.1 A turn-context block instead of a growing system prompt

`moim.rs` adds, to each request, a tagged block with the current time, the
working directory, the turn budget and anything extensions contribute. The
system prompt explains the tag once: it is not part of the user's request,
only the newest block is current, and as the turn budget gets low the model
should explore less, batch calls and finish. The block is skipped when the
model's window is under 32,000 tokens, where it would cost too large a
share.

Because the block is appended at the end, the cached prefix is untouched.
KodAct states the time limit up front and sends one reminder near the end;
the model does not otherwise know how much budget is left.

### 2.2 Compaction as structured data

The summarizer is asked for an `<analysis>` scratchpad (told to keep it
brief, since it is discarded) and then one JSON object: user intent,
technical concepts, files (path, what was done and why, key code), errors
and fixes, problem solving (what was chosen, what was rejected, why), user
messages, pending tasks, current work, next step. The rules:

- quote error messages and failing test output word for word, with
  numbers, identifiers and paths;
- the reader is the model itself, so the summary may be long;
- order every list by importance;
- omit a field instead of inventing content;
- "next step" only if it continues a user instruction.

The harness then renders the JSON through a template the person can
override (to keep only the three most important goals, say) without
rebuilding. If the summary request does not fit, the middle of the
conversation is removed in steps (0, 10, 20, 50, 100 percent) until it
does.

After compaction the model is told its context was compacted and **not to
mention it**; a different sentence is used when it was mid-tool-loop
("continue calling tools as necessary").

The threshold is 80% of the window, capped at 225,000 tokens, with a
comment giving the reason: one provider charges more above a certain input
size, so compaction starts before that tier. That is a cost rule read from
the price list, not from the window.

### 2.3 Summarizing old tool calls one batch at a time

An optional mode (`GOOSE_TOOL_PAIR_SUMMARIZATION`) replaces old tool call
and result pairs with a short model-written summary of each, ten at a
time, once enough of them lie beyond a cutoff. It is a middle step between
clearing a result (KodAct's micro-compaction) and summarizing everything:
the model keeps a line about what each old call found.

It costs model calls on the way, which is why it is off by default there.

### 2.4 Large tool responses

A tool result over 200,000 characters is written to a file and replaced by
a message that gives the size and the path. Shell output keeps 2,000 lines
or 50 kB and saves the rest. KodAct's budgets do the same.

### 2.5 A reviewer for shell commands, written by the person

`adversary_inspector.rs`: if the person places an `adversary.md` file in
the config directory, each shell command is sent to a model with the
person's rules and the last four user messages, and blocked on a "block"
verdict. With no file the inspector is off. If the review fails, the
command is allowed. The default rules end with "err on the side of allow".

This is the cheap form of Codex's auto-review: opt-in, rules in the
person's words, fail-open.

### 2.6 Structured final output and success checks

Recipes (headless runs described in YAML) can declare:

- a **response schema**: a `final_output` tool is added, the run does not
  end until it is called alone, with JSON matching the schema, and the
  result is printed on one line for scripts to parse;
- **success checks**: shell commands run after the agent finishes; if one
  fails, an `on_failure` command runs and the whole recipe is retried, up
  to a limit, each with a timeout.

Success checks are verification independent of the agent, inside the
harness: the same idea as `kodact-eval`'s hidden checks, offered to people
for their own scripts.

### 2.7 Smaller points

- Shell commands run with `AGENT=goose` in the environment, so a project's
  scripts can tell an agent from a person (skip a pager, print plain
  output).
- The edit tool's "not found" error includes a "did you mean" block with
  the most similar text.
- Tool schemas from MCP servers are normalized before being sent to
  providers that reject parts of JSON Schema.
- A repetition monitor refuses the same tool call past a configured count.

## 3. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| S1 | `--check <command>` (and a settings key) for `-p` runs: after the turn ends, run the person's command; on failure, give its output back to the model, up to N times; the result records pass or fail | A script gets "done and the check passed" from one command, with the retry built in | Verification / false-finish rate | Model-neutral; the check is the person's | None unless the flag is given |
| S2 | `--json-schema` exists (`docs/PARITY.md`: "no retry on invalid output yet"). Add the retry: an answer that fails the schema is sent back with the validation error, up to N times, and the result says so when it never validates | Scripts get valid JSON or a clear failure, not a parse error | Compatibility / usable results (the PARITY row) | Rarely triggers | None unless the flag is given and the answer is invalid |
| S3 | Set `AGENT=kodact` (and `KODACT=1`) in the environment of commands | A project's scripts can skip pagers and colour for the agent | Tool design / hangs and noise in output | Same | None |
| S4 | The summary instruction asks for errors and failing output word for word, and for rejected approaches with the reason | After compaction the model does not retry an approach it already ruled out | Context management / turns after compaction | Same | None before compaction |
| S5 | Tell the model the remaining budget in a small block on the newest message when a budget or time limit is set | Budgeted runs pace themselves instead of getting one late warning | Long-horizon / pass rate on `optimize-under-time` | Only in budgeted runs; a few tokens at the end of the request, cache untouched | None in sessions without a budget |
| S6 | An opt-in reviewer for shell commands with rules in the person's file, fail-open | People who run unattended can state "never push, never touch ~/.ssh" in their own words | Compatibility / blocked dangerous commands | Same | Off by default; a model call per reviewed command when on |
| S7 | A text protocol for models that cannot produce tool calls | A very small local model can still run commands | Tool design / invalid-call rate | Never used there | None; chosen only after tool calls fail repeatedly |

S7 extends what KodAct already does (it runs a tool call written as
text). The difference is telling the model the simpler form up front once
its tool calls have failed several times, a decision made from behaviour
in the run, as `docs/goal.md` requires.
