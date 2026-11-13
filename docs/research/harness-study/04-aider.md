# Aider: what its source teaches KodAct

- **Source:** `Aider-AI/aider` at commit `5dc9490` (2026-05-22, the head of
  the default branch on the day of reading: the repository has been quiet
  for six months).
- **Licence:** Apache-2.0 (`LICENSE.txt` checked). Adaptation with
  attribution is allowed. Nothing is copied here.
- **Read in full:** `aider/coders/base_prompts.py`, `editblock_prompts.py`,
  `architect_prompts.py`, `ask_prompts.py`, `aider/prompts.py`,
  `aider/history.py`.
- **Read in part:** `coders/editblock_coder.py` (matching and error text),
  `coders/base_coder.py` (the reflection loop), `aider/linter.py`,
  `aider/repomap.py` (ranking), `aider/models.py` (per-model settings),
  `coders/chat_chunks.py`.
- **Not read:** the other edit formats (udiff, whole file, patch), the
  commands, the voice, GUI and watch features, the benchmark harness.

## 1. A different design

Aider does not use tool calls. The person adds files to the chat, their
full text is placed in the prompt, the model answers with edit blocks in a
text format, and Aider parses and applies them, then commits. There is no
agent loop exploring the repository: the person, helped by a repository
map, decides what the model sees. It is included in this study because the
parts that are *not* about that design are the most battle-tested in the
field: Aider's edit matching and model settings come from years of public
benchmark runs across hundreds of models.

## 2. How the prompt is built

A coder class per edit format carries its prompt pieces. `chat_chunks.py`
orders them for caching: system text, examples, read-only files, the
repository map, the summary of old conversation, the files being edited,
the current messages, and a reminder at the end. Cache marks go on the
examples (or system), the repository map and the edited files, in that
order of stability.

Three ideas in the prompt text:

1. **Two behaviour flags per model.** `lazy` adds three lines telling the
   model to implement code fully and never leave a comment in its place.
   `overeager` adds three lines telling it to do what was asked and nothing
   more. `model-settings.yml` sets them per model, from benchmark results.
   The observation behind it: models fail in one of two opposite
   directions, and one prompt cannot correct both.
2. **The rules are repeated at the end.** The edit-format rules go in the
   system prompt and again as a reminder after the conversation
   (`reminder: user` or `sys`, per model), because adherence to a format
   decays over a long context.
3. **The current file wins.** The files message says to trust it as the
   true contents and that other messages may hold outdated versions. After
   an edit Aider tells the model, as the user, "I committed the changes
   with git hash …", so the model does not re-send them.

Point 1 is a per-model-name table, which `docs/goal.md` rules out ("never
from its name or size"). The finding still transfers: KodAct's prompt holds
both corrections at once ("keep changes to what was asked" and "never swap
in a stub"), which is the model-neutral form.

## 3. Mechanisms worth knowing

### 3.1 Edit matching, in order

`editblock_coder.py` applies a search block by trying:

1. the exact lines;
2. the same lines with the leading whitespace uniformly off (the model
   dropped or shortened all indentation in both search and replacement):
   the missing indentation is found from the file and added to the
   replacement;
3. the same after dropping a spurious blank first line;
4. a block that elides code with `...` lines, when the pieces around the
   dots match exactly;
5. the same block in another file of the chat, when it fails in the file
   named.

A path given slightly wrong is resolved against the known files: exact,
then by base name, then by fuzzy match.

When all fail, the error is written for the model: the failed block, "did
you mean to match some of these actual lines" with the most similar lines
(similarity over 0.6), a note when the replacement lines are **already in
the file**, and a count of the blocks that did apply with "don't re-send
them". Partial success is kept, not rolled back.

Step 2 is the one KodAct lacks and the most common failure of small
models. Gemini CLI's "flexible" strategy does the same thing; opencode and
Aider both report "already in the file". Three independent projects
arrived at the same two fixes.

### 3.2 Lint, then test, as the loop

After every applied edit Aider lints the changed files and, if configured,
runs the test command. Errors are sent back as the next message, at most
three times (`max_reflections = 3`), then it stops and reports.

The linter (`linter.py`) needs nothing installed for most languages: it
parses the file with tree-sitter and reports syntax error nodes. For
Python it also compiles the file and runs a fixed set of fatal checks
(undefined names and similar), not style. Each error is shown inside its
enclosing scope: the function or class header lines around the bad line,
with the rest elided, so the model sees where it is without a full read.

KodAct checks syntax "by the language's own parser when installed". On a
machine without that toolchain (a small container, a fresh laptop), the
check is silent. A parser built into the binary would make the check
independent of what is installed.

### 3.3 The repository map

`repomap.py` builds a ranked outline of the repository within a token
budget (1,024 by default; eight times that when no file is in the chat):

- tree-sitter extracts definitions and references per file, cached by
  modification time;
- a graph links each file that references a name to the files that define
  it; edges weigh more for names the person mentioned (×10), for long
  well-formed identifiers (×10), from files in the chat (×50), and less for
  private names (×0.1) and names defined in many places (×0.1);
- PageRank, personalized toward the chat files and the files and names the
  person mentioned, ranks definitions;
- a binary search finds how many top definitions fit the budget, and they
  are printed as signature lines under their file names.

The effect: a model that has never seen the repository is told, in about a
thousand tokens, which files and functions matter for this request.

For an agent with Grep and Glob the map is less necessary, and it costs
tokens on every request, which rule 9 of `docs/goal.md` weighs against. Its
natural place in KodAct is on demand: an outline the model can ask for, or
that the Explore agent starts from.

### 3.4 Summarizing by halves

`history.py` keeps the newest messages that fit half the history budget,
summarizes the older half, and if the result is still too large, recurses
(depth at most three). The summary is written in the first person as the
user ("I asked you…") so it reads as part of the conversation, and must
name the functions and files discussed. A weaker, cheaper model writes it,
with the main model as a fallback.

### 3.5 Architect and editor

In architect mode a strong model describes the change in prose and a
second, cheaper model turns the description into edit blocks. The
architect is told the editor relies only on its instructions. On Aider's
own benchmark this pairing scored above either model alone. KodAct's
equivalent would be a Task agent that receives a precise change
description; nothing needs building, but the pairing is a configuration
worth an eval.

### 3.6 Weak model for side jobs

Commit messages and summaries go to `weak_model_name`. The commit prompt
is twelve lines: one line, conventional prefix, imperative, at most 72
characters, nothing else.

## 4. Candidates for KodAct

| # | Change | In real use | Pillar / metric | Large model | Cost on a quiet turn |
| --- | --- | --- | --- | --- | --- |
| A1 | Edit applies a match whose only difference is a uniform indentation offset, fixing the replacement's indentation to the file's (same as Gemini CLI G2) | The most frequent failed edit on small models costs no turn | Tool design / invalid-call rate, turns | Runs only after the exact match fails | None |
| A2 | A multi-edit call keeps the edits that applied and reports only the failed ones, saying not to re-send the rest | A five-part edit with one bad part costs one small retry, not five | Tool design / turns after first error | Same | None |
| A3 | A syntax check built into the binary (tree-sitter) for the common languages, used when the language's own tool is not installed | A broken edit is reported in a container with no toolchain | Verification / `edit_diagnostics`, false-finish rate | Same | A parse of the edited file, a few milliseconds; nothing said when it parses |
| A4 | Show an edit error inside its enclosing scope (the function or class header lines) | The model sees where the error is without re-reading the file | Verification / turns after an edit error | Same | Only when an error is reported |
| A5 | A repository outline on demand (a tool, or the Explore agent's first step), ranked by references and by the names in the request | A question about an unfamiliar repository starts from the right files | Context management / turns on `map-the-routes`, `explain-setting` | A strong model may never call it; then it costs one tool definition | About 300 characters of tool definition; A/B before default-on |
| A6 | Route the summary call to the fallback or a configured cheaper model | Compaction costs less | Context management / cost per task | Only where a second model is configured | None |
| A7 | Eval: a strong model writes the change description, a Task agent on a cheaper model applies it | A cheaper way to make large mechanical edits | Long-horizon / cost per passed task | That is the case it is for | Configuration only |

Not proposed: per-model `lazy`/`overeager` flags (a table keyed on model
names), full file contents in the prompt, text edit formats.
