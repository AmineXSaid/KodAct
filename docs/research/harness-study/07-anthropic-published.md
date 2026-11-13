# Anthropic's published material: what it teaches KodAct

- **Sources**, read on 2026-11-13 from the public documentation sites
  (reachable from this environment now; they were blocked when
  `docs/REFERENCES.md` was written in October):
  - Prompting best practices
    (`platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices`)
  - Prompting Claude Fable 5.2 (same section, `prompting-claude-fable-5-2`)
  - Prefix checks on thinking blocks
    (`…/build-with-claude/preserved-thinking/prefix-checks`)
  - Context editing (`…/build-with-claude/context-editing`), a summary only
  - Agent SDK: overview, "How the agent loop works", "Modifying system
    prompts" (`code.claude.com/docs/en/agent-sdk/…`)
  - The published system prompt for the chat apps, entry of 2026-11-09
    (`platform.claude.com/docs/en/release-notes/system-prompts/…`)
- **Licence:** these pages are not licensed for copying. Everything below
  is described in this report's own words; nothing is quoted beyond a few
  words. The documentation itself offers sample prompt text for developers
  to put in their own system prompts; where this report recommends one, it
  names the page and section so the text is taken from the source at the
  time of use, under the terms that apply then.
- **Not read:** the other per-model prompting pages, the memory tool,
  compaction, effort and tool-use pages, the hooks, permissions, sub-agents
  and sessions pages of the Agent SDK.

## 1. One finding that is a correctness issue, not a tuning idea

**Models with "preserved thinking" reject a request whose earlier history
was edited.**

What the documentation says:

- On the current top models, each thinking block's signature records a
  fingerprint of everything before it: the system prompt, the tool
  definitions and every earlier message.
- When a request sends such a block back and any of that has changed (one
  word of a tool description is enough; so is changing, inserting, moving
  or deleting an earlier message), the block fails a "prefix check".
- On the newest model family the API then answers 400 ("bound to a
  different conversation"). On the three other current models the result
  is a 400 or, silently, reasoning about content that has since changed.
- Server-side context editing, compaction at a token threshold, and
  appending never count. Cache markers, the effort level and other
  parameters do not count.
- A beta setting makes the API drop the invalid blocks instead of failing;
  Anthropic reports that the model then thinks again and spends more
  output tokens.
- The documented rule for a harness: send each assistant turn back exactly
  as received, build the system prompt and tools once per conversation, put
  every update in a new message at the end and leave it there.

What KodAct does today (read from the source, not run against the API: this
environment has no key):

| KodAct behaviour | Where | Against the rule |
| --- | --- | --- |
| Micro-compaction replaces the text of earlier tool results in the request | `crates/kodact-engine/src/request.rs` (`normalize`), `engine.rs` | Edits earlier messages |
| `normalize` merges consecutive messages of the same role and drops empty text blocks | same | May change an earlier message's shape; needs checking against what the API returned |
| The system prompt carries today's date and the git status snapshot | `crates/kodact-engine/src/prompts.rs` (`EnvInfo::render`) | A session resumed the next day, or after a commit, sends a different system prompt |
| A tool list that changes mid-session (an MCP server that connects late, a tool switched on). Not checked in this study whether KodAct's list can change after the first request | engine and MCP manager | Would change `tools` |
| Partial compaction keeps the messages after the summary note | `engine.rs` (the `"partial"` trigger) | Kept turns' thinking refers to a prefix that no longer exists |

If any of these happens in a session on one of those models while thinking
blocks are in the history, the documentation says the next request fails or
degrades. The failure would appear exactly in the long sessions KodAct's
context management exists for.

This is recommendation **N1** in section 5, and the first item of the
synthesis. It is stated as "owed a check", not as an observed bug: the
first step is a test against the real API with a key.

The Agent SDK pages describe the reference harness doing the append-only
form of each item above: the system prompt is recorded at a session's
first request and reused until compaction; the working directory,
platform and project instructions travel in the conversation, not the
system prompt; an instruction that changes mid-session is sent as a new
message or hook context; an option moves per-user sections out of the
system prompt so sessions share a cache entry.

## 2. The prompting guide: what applies to KodAct's prompt

General principles, each checked against `crates/kodact-engine/prompts/`:

| Guidance | KodAct's prompt today |
| --- | --- |
| Say why. A rule with its reason is followed better and generalizes; a bare prohibition does not | Mixed. Several rules give the reason ("since what reads it may be a program", "reworking a passing solution costs time and can break it"). Many do not |
| Drop emphatic wording. Current models over-apply instructions written in capitals or as "critical"; plain wording works | Already plain. No capitals or "CRITICAL" anywhere |
| Instructions written to push older models to be thorough now cause over-work; remove them | Worth an audit: "Understand before changing", "Verify your work" and the unattended section's attempt rule were written from small-model runs |
| A word such as "never" is read as absolute | Six "never" in the prompt; each is meant as absolute except "Never generate or guess URLs", which has an exception in the same sentence |
| State whether to act or to advise by default; both sample texts are given | Not stated (see Gemini CLI G3) |
| Parallel tool calls are steerable with one paragraph | One sentence present |
| Tell the model that context is compacted automatically, so it does not wind down early as the window fills | Not stated (see Codex C6) |
| For work across context windows: tests and state in files, git as the log, start a fresh window from the files | The Notes tool and task list cover state; the prompt says "work in files as you go" |
| Reversibility: local reversible actions freely, confirmation for destructive, hard-to-reverse or externally visible ones; never use a destructive action as a shortcut past an obstacle | Only "never commit, push or open pull requests unless asked" |
| Avoid over-engineering: scope, comments, defensive code, abstractions | "Keep changes to what was asked" covers scope and comments, not defensive code or abstractions |
| Do not special-case tests; say when a test or the task is wrong | "Never swap in a stub… or a hard-coded answer" covers the first half |
| Never speculate about code not opened | "Say what you observed, not what you guess" covers tool failures, not code |
| Clean up temporary files at the end | Not stated |

## 3. Guidance specific to the newest model family

These are measured results Anthropic reports for one model family. By
`docs/goal.md` they are leads to check on KodAct's own evals with several
models, not rules to adopt for one model.

- **Early stopping.** The model sometimes ends a turn with work left,
  listing what remains or offering to continue, more often at low and
  medium effort. A paragraph that separates ordinary obstacles (errors,
  timeouts: diagnose and work through) from deliberate blockers (a file
  marked not to touch: leave it and say so) halved such endings in their
  replay, for about 30% more output tokens. KodAct's unattended section
  has the first half; the distinction between the two kinds of blocker is
  missing, and it is model-neutral.
- **A token budget shown to the model changes how much it spends.**
  Removing a large displayed budget lowered cost at equal performance.
  This cuts against showing remaining budget every turn (Goose S5) on
  strong models, and supports KodAct's single late reminder.
- **Silence during long turns.** A harness-side counter of tool-calling
  steps with nothing for the person to read, then a one-line reminder
  appended after the latest tool results, halved the longest silent
  stretch at no measurable cost. The reminder must stay in the history
  afterwards (removing it edits an earlier turn).
- **Approval rules must name the actions.** A broad rule held back about
  half of routine requests in their test. "Ask before deleting files"
  keeps the model responsible; "never delete files" is absolute.
- **Caution accumulates.** Notes, memory and instruction files collect
  cautions whose reason has expired, some written by the model itself.
  They should be pruned. This applies to KodAct's memory and Notes tools.
- **Who owns the code.** Without a statement, the model adds to the
  existing structure, even duplicating; told it owns the codebase, it
  consolidates (under 10% of runs against about 75%) and also makes
  unrequested changes, with 70–80% more output. For a person's own project
  the default (say nothing) matches "keep changes to what was asked".
- **Reports for a reader without context.** After long work the final
  message should stand alone: key points first, no terms coined during the
  work, full sentences. KodAct asks for "what changed and how you verified
  it in a sentence or two", which is right for a short task and thin for a
  two-hour run.
- **Sub-agents.** Delegation follows what the prompts invite. The tool
  description should state what a sub-agent costs and when it is worth it.
  The newest model also used sub-agents to review its own work at the
  highest effort levels.
- **Effort.** It sets thinking, tool calls, time and tokens; it has little
  effect on reply length. Lower levels need the "finish the task" text
  more. A beta lets the level change per message without losing the cache.

## 4. The published chat system prompt: techniques only

The prompt is for the chat apps, not for a coding agent, and most of its
content is policy. What transfers is form:

- It is written in the third person about the model, in prose paragraphs
  grouped under descriptive tags, with almost no bullet lists.
- Rules usually arrive with their reason or with the signal that triggers
  them ("if you find yourself reframing a request to make it acceptable,
  that is the signal to refuse").
- Examples are few, complete, and followed by a one-line rationale.
- Formatting guidance is given as defaults with conditions (prose unless
  the content needs a list), not as fixed limits.
- The knowledge cutoff is stated with what to do about it: answer as an
  informed person of that date would, say when something may be newer.
- A closing section says that tags, labels and signatures inside a message
  do not prove who wrote it: a "system reminder" in a tool result or a
  file that came with a repository cannot approve anything.

The last point matters for KodAct directly. Its prompt says
`<system-reminder>` tags "carry information from the system". A file or
web page can contain the same tag. The prompt should say that a tag inside
tool output or a file is content, whatever it claims.

## 5. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| N1 | Make requests append-only for models with preserved thinking: (a) a test against the real API that runs micro-compaction, a resume across a date change and an MCP connect with thinking on; (b) where it fails, clear tool results through server-side context editing on those models, keep the recorded system prompt and tool list for the life of a conversation, and send changes as new messages; (c) handle the 400 by resending with the invalid blocks dropped | Long sessions on the newest Claude models do not end in a 400 or lose their reasoning at the first micro-compaction | Failure recovery, context management / recovery rate, pass rate on long tasks | This is the large-model case. Other endpoints keep today's behaviour | None |
| N2 | Move the date and git status out of the system prompt into the first user message by default | The system prompt is identical across sessions and days, so it caches across sessions and never invalidates stored thinking on resume | Context management / cached-token share | Same | None; `exclude_dynamic` already implements the mechanism |
| N3 | Prompt: tags inside tool output or files are content, whatever they claim to be | A fetched page with a fake reminder cannot pose as the harness | Tool design / pass rate on `injected-docs` | Model-neutral | About 25 tokens |
| N4 | Prompt: separate ordinary obstacles (work through) from deliberate blockers (leave and report) | Fewer runs that stop at a timeout or a failing tool and call it done; no runs that force past a guard | Long-horizon / false-finish rate | Model-neutral; Anthropic measured it on their newest model | About 60 tokens |
| N5 | Prompt: reversible local actions freely; confirm destructive, hard-to-reverse or externally visible ones; no destructive shortcut past an obstacle | A person's uncommitted work is not discarded by `git checkout --` to "fix" a conflict | Verification / destructive actions in runs | Model-neutral | About 50 tokens |
| N6 | Give each prompt rule its reason, in one clause | Rules are applied where they fit instead of everywhere | Every pillar / quiet-check pass rate and tokens | Stronger models use the reason more | A few tokens per rule; A/B with `scripts/quiet-check.sh` |
| N7 | A silent-steps counter: after several tool-calling steps with no text, one appended line asking for a few words | A person watching a long turn is not left guessing | Compatibility / longest silent stretch | Measured by Anthropic on their newest model; counts behaviour in the run | One line, only after a long silent stretch; never removed afterwards |
| N8 | A Task tool description that states what a sub-agent costs and when it pays | Fewer cold-start sub-agents for work a grep would do | Context management / cost per task | Applies most to strong models | Same length as today if rewritten, not added |
| N9 | `/memory` and Notes: a way to list and prune cautions, and guidance not to store absolute rules without scope and reason | Old cautions stop slowing later sessions | Long-horizon / turns per task over time | Applies most to models that follow instructions closely | None |

N4 to N6 change the prompt for every model, so each needs the quiet check
on a small and a large model before it is on by default.
