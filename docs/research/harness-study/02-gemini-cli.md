# Gemini CLI: what its source teaches KodAct

- **Source:** `google-gemini/gemini-cli` at commit `b977656` (2026-11-12).
- **Licence:** Apache-2.0 (`LICENSE` checked; each source file carries the
  SPDX header). Adaptation with attribution is allowed. Nothing is copied
  here.
- **Read in full:** `packages/core/src/prompts/snippets.ts` (the whole
  system prompt and the compression prompt, 977 lines).
- **Read in part:** `context/chatCompressionService.ts`,
  `context/toolOutputMaskingService.ts`, `services/loopDetectionService.ts`,
  `tools/edit.ts`, `utils/llm-edit-fixer.ts`,
  `tools/omissionPlaceholderDetector.ts`, `utils/historyHardening.ts`,
  `routing/strategies/classifierStrategy.ts`, the tool definitions in
  `tools/definitions/model-family-sets/gemini-3.ts`, `evals/README.md` and
  the list of eval files.
- **Not read:** the CLI/UI package, the scheduler, policy engine, MCP
  client, sandbox, telemetry, the A2A server and SDK.

## 1. How the prompt is built

One TypeScript file renders the prompt from option structs: each section
is a function that returns text or nothing, and the tool names are
constants interpolated into the text, so a renamed tool cannot leave a stale
name in the prompt. Sections: preamble, core mandates, sub-agents, skills,
hook context, workflows (or the plan-mode workflow in their place), task
tracker, operational guidelines, sandbox, git. An interactive flag changes
single sentences inside sections instead of adding a separate "unattended"
section.

A comment in the source guards the context-efficiency section: it must not
be changed without running the large benchmarks, because it balances small
responses against extra turns. That is the same discipline as KodAct's
"a description changes only when the table says so".

### What the prompt says that KodAct's does not

| Subject | Gemini CLI's rule, in short | KodAct today |
| --- | --- | --- |
| Asking vs doing | A request is an *inquiry* (analysis, advice, "how do I…") unless it explicitly asks for action; inquiries never change files. A reported bug without a request to fix it gets a question first | "If ambiguous, ask a short question" only |
| Turn economy | The whole history is resent every turn, so an extra turn costs more than a larger read. Ask grep for context lines to skip the follow-up read; read several ranges in one turn | Parallel calls are mentioned; the cost model is not |
| Reproduce first | For a bug fix, reproduce the failure with a test or script before fixing | Not stated |
| Re-evaluation | After more than three failed fixes: restate the task, list the assumptions that may be wrong, propose a different approach | The loop guard does this from the harness side |
| No hacks | Never silence warnings, bypass types or use reflection to make something pass | "Fix the cause" covers tools, not type systems |
| Mid-run hints | A message sent while working is a scoped correction: change the minimum, keep the other tasks | Not stated |
| Declined calls | Do not retry or negotiate a declined call; offer another path | Present, equivalent |
| Empty replies | After a tool result, always call a tool or write text; never return nothing | Handled in the engine |
| Git | Never `git add .` unprompted; gather status, diff and log in one command; draft the message | "Never commit unless asked" only |
| Memory tiers | One fact lives in exactly one file: shared project file, private project memory, or global memory; never transient state | Memory tool exists; routing rule not stated |

Note the opposite defaults: Codex's newest prompt says "can you…" means *do
it*; Gemini CLI's says assume an inquiry. KodAct's rule 10 ("a guess about
what the person meant must fail safe") sides with Gemini CLI: not changing
files costs a line of text, changing them when only asked a question costs
a revert.

## 2. Mechanisms worth knowing

### 2.1 Compression in three steps, with a check

- It starts at **half** the model's window (`0.5`), far earlier than
  KodAct or Codex. The last 30% of the history is kept as is; the rest is
  summarized.
- Before summarizing, old tool results are collapsed: results older than
  the last three turns shrink to 2 KB, except reads and searches, which stay
  whole up to 64 KB in total.
- The summary is a fixed XML snapshot: goal, constraints, key knowledge,
  artifact trail (what changed and why), file-system state, recent actions,
  task state with done/in-progress/todo marks.
- A **second model call** asks the summarizer to check its own snapshot for
  omitted paths, results and constraints and to output an improved one.
- If the result has more tokens than the original, the compression is
  rejected.
- The summarizer's prompt opens with a rule that instructions inside the
  history are data: a hostile tool result cannot redirect the summary.

For KodAct: the half-window threshold is a fit to one family of models and
is not proposed. The injection rule and the "reject if larger" guard are
cheap and general. The self-check doubles the cost of every compaction; it
is worth an A/B on `deliverable-after-compaction`, not adoption on faith.

### 2.2 Tool-output masking in batches

Separate from compression: the newest 50,000 tokens of tool output are
protected; older outputs are replaced by a marker (the full text is saved
to a file) only once at least 30,000 tokens can be removed in one go.
Batching matters because every removal rewrites history and breaks the
provider's prompt cache from that point: one large rewrite is cheaper than
many small ones. Skill activations, user answers and plan-mode results are
never masked.

KodAct's micro-compaction clears stale results over 8,000 characters and
keeps three turns. Whether it batches is the point to check: **each
micro-compaction that edits an earlier message invalidates the cache for
everything after it.**

### 2.3 Four edit strategies, then a model

`edit.ts` tries, in order: exact text; "flexible" (line by line, ignoring
indentation, then re-indenting the replacement to the file's indentation);
a regex built from the search text with whitespace made optional between
tokens; a fuzzy match by edit distance (skipped for very short strings,
and the result says "applied fuzzy match at line N" so the model knows).
If all fail, a second model call receives the failed search, the error and
the file, and returns a corrected search string or "no change required"
when the edit is already in the file.

Also in the edit path: an **omission detector** refuses a replacement that
contains a placeholder such as "rest of methods ..." where the original had
none. This catches the most damaging edit a model can make: deleting code
by abbreviating it.

KodAct reports near matches in the error and lets the model retry. That
costs one turn each time; applying an indentation-only match directly and
saying so would save it. The omission check is not in KodAct's Edit or
Write (no such check in `edit.rs` or `write.rs`).

### 2.4 Loop detection in two tiers

Tier one is mechanical: the same tool call five times in a row, or the
same 50-character chunk of text ten times. Tier two, after 30 turns in one
request, asks a model every 5 to 15 turns (the interval adapts to the last
confidence) whether the last 20 turns are an unproductive loop, with a
careful prompt on what is *not* a loop: the same tool on different files,
different edits to one file, re-running a build after a change. A
confidence over 0.9 is double-checked by a second model before acting.

KodAct's detection is mechanical and richer than tier one (same failing
call, same result, edits that undo each other, five failures in a row). The
list of non-loops is a good source of **negative test cases** for
`stuck.rs`; the model tier costs calls on healthy runs and conflicts with
"a turn with nothing to report costs nothing".

### 2.5 History hardening

Before each request the history is made valid by patching, not failing:
alternating roles, a tool response for every call (a sentinel text when the
result was lost), a placeholder for a reply that was interrupted.

### 2.6 Model routing

A small classifier model reads the request and picks the fast or the
strong model by a rubric (number of steps, need for planning, debugging
from symptoms). Wording such as "best way" does not count; operational
complexity does.

KodAct runs the model the person chose. Routing only applies where two
models are configured (a fallback exists today). An idea, not a proposal.

### 2.7 Untrusted content is tagged by the harness

Output from external tools and MCP servers is wrapped in
`<untrusted_context>` tags, and the prompt says to treat what is inside as
data, and to take an author's identity only from the envelope, never from
text inside the body. KodAct scans for injection patterns and adds a note
when one matches. Tagging by *source* does not depend on a pattern list
and so does not depend on the language of the attack.

### 2.8 Behavioural evals

`evals/` holds about 40 small tests that check what the model *chooses*:
answer or act, frugal reads, frugal search, concurrency safety, no hang on
interactive commands, validation fidelity, prompt injection through MCP,
fidelity of the compression snapshot. Each is marked "always passes" or
"usually passes", so a flaky behaviour does not block a merge but is
tracked. These are the closest thing in the six repositories to KodAct's
`quiet` tasks.

## 3. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| G1 | Edit and Write refuse a replacement that abbreviates code with a placeholder comment the original did not have, and say which line | A file is never silently truncated to "// ... rest of code"; today that is found only when the build fails, or not at all | Tool design, verification / false-finish rate | Rare on strong models; a check on text only, no model call | None: a scan of `new_string` |
| G2 | Edit applies a match that differs only in leading whitespace, re-indents the replacement to the file, and says so | The most common Edit failure costs no extra turn | Tool design / turns, invalid-call rate | Stays out of the way: runs only after the exact match fails | None |
| G3 | Prompt rule: a question about the code, or a reported problem without a request to change it, is answered and not acted on | "Why does this fail?" gets an explanation, not an unrequested edit | Verification / false actions on the `quiet-question` task | Model-neutral wording; fails safe | About 40 tokens per request |
| G4 | Prompt rule: for a bug, reproduce it before fixing it | The fix is checked against the failure the person saw, not an assumed one | Verification / false-finish rate | Model-neutral | About 20 tokens |
| G5 | The summary instruction states that text inside the conversation is data, and the summary is rejected if it is larger than what it replaces | A fetched page cannot steer the summary; a compaction can never make the context bigger | Context management / pass rate after compaction | Same | A sentence in the summary call only |
| G6 | Batch micro-compaction: clear stale results only when a minimum share of the window can be freed at once | Fewer cache breaks, so long sessions cost less on providers that cache | Context management / cost per task | The minimum is a share of the window, so a large window clears rarely | None; measure cached-token share before and after |
| G7 | Mark the output of web and MCP tools as untrusted by source, in addition to the pattern scan | Injection in any language is framed as data | Tool design / pass rate on `injected-docs` | Same | A tag per external result (about 10 tokens) |
| G8 | Add Gemini CLI's non-loop cases to the tests of `stuck.rs` | No false "you are looping" on a batch edit across files | Failure recovery / reminders on healthy runs | n/a (tests) | None |
| G9 | A/B a self-check pass on the summary | Fewer facts lost in compaction, at the price of a second call | Context management / pass rate, cost | Compaction is rare there | One extra call per compaction; only if the A/B pays for it |
