# KodAct architecture

KodAct is a clean-room Rust agentic coding CLI. Its behaviour follows a
reference CLI, version **2.1.290** (the 2026-10-06 release). The only sources
are public interfaces: the reference CLI's `--help` output (2.1.291 was the
binary on the build machine, used as the flag reference), the documented SDK
stream-json protocol, and the Messages API documentation. No leaked or
proprietary source is used, and no third-party prompt text is committed (see
"Prompts" below).

## Crates

Dependencies point one way. Nothing lower in the list depends on anything
higher.

```
kodact-platform                   OS differences: the shell commands run in, process trees, path display
kodact-types                      wire types (Messages API + stream-json protocol)
  ├─ kodact-api                   Provider trait, Messages API SSE client, OpenAI-compatible adapter, mock, model table + pricing
  ├─ kodact-config                settings layers, memory files (KODACT.md), resource search paths
  ├─ kodact-permissions           modes, rule grammar, decision engine
  ├─ kodact-hooks                 hook events, matchers, command runner
  ├─ kodact-session               JSONL transcripts, resume / continue / fork, file checkpoints + rewind
  └─ kodact-git                   repo detection, status for the prompt, worktrees
kodact-tools                      Tool trait, registry, built-in tools
  ├─ kodact-compact               auto-compact, /compact, micro-compaction of stale tool results
  └─ kodact-mcp                   MCP client (stdio / HTTP / SSE) and `kodact mcp serve`
kodact-engine                     the query loop: stream a turn, batch tools, permission → hook → execute → hook, interrupt, budgets, fallback model
kodact-agents                     Task sub-agents, custom agents, skills, slash commands, output styles, plugins
kodact-core                       composition root: builds an engine from config + flags; ports (PermissionPrompter, EventSink)
  ├─ kodact-cli                   the `kodact` binary (headless print mode, stream-json host protocol, subcommands)
  └─ kodact-tui                   interactive terminal UI
kodact-test-host                  test-only SDK host: drives `kodact` over stream-json the way an IDE extension does
```

## Behavioural contracts

These are fixed before the engine is written. Each has a named test (see
`PARITY.md`).

### C1. Headless permission prompts

In `-p` mode a tool call that resolves to **ask** goes to whoever answers
prompts:

| Configuration | What happens to an `ask` |
| --- | --- |
| `--permission-prompt-tool stdio` (SDK host), `--permission-prompts host` (default) | A `can_use_tool` `control_request` goes to the host. Its `allow` / `deny` answer is final. |
| `-p` with no host, or `--permission-prompts none` | **Denied automatically.** The model receives an `is_error` `tool_result` that names the tool and says how to allow it (`--allowedTools`, a permission mode, or settings rules). The denial is recorded in `result.permission_denials`. The turn continues; it is not failed. |

Read-only tools inside the working directories are allowed by default rules, so
`kodact -p "explain this repo"` works with no flags. `bypassPermissions` skips
the prompt but not explicit **deny** rules.

### C2. Concurrency, permissions and hooks

The tool calls in one assistant message run as follows:

1. They're split into consecutive batches. A batch is either a run of
   concurrency-safe tools (read-only: Read, Glob, Grep, LS, WebFetch, ...) or a
   single unsafe tool.
2. **Within a batch every tool runs its own pipeline concurrently:**
   permission check → `PreToolUse` hook → execute → `PostToolUse` hook.
3. **Permission prompts are serialized** through one async mutex, so a person or
   host never sees two prompts at once. Checks that need no prompt don't take
   the mutex.
4. Results go back in the order of the `tool_use` blocks, all in one user
   message. The executor puts each result into the slot of its `tool_use`
   index, so concurrent tools that finish out of order are put back in order.
   Batches run strictly one after another, so a batch boundary can't reorder
   results either.

   Test: `engine::c2_result_order_survives_concurrency`, which runs
   3×Read, Edit, then 2×Read, with deliberately inverted completion times.

That's model (c) from the design review. Running each pipeline per tool means a
prompt sees the filesystem as it is when that tool is about to run (no
check-everything-first TOCTOU window). Serializing the prompts keeps the
TUI/host dialog to one at a time.

### C3. Interrupt

There is one mechanism for interrupting: a `tokio_util::sync::CancellationToken` per
turn, owned by the engine. Esc in the TUI and the `interrupt` control request
both cancel it.

- **The model stream** is dropped at once. The partial assistant message is
  kept if it has any text or complete tool calls; incomplete tool calls are
  discarded.
- **In-flight tools** are aborted, not awaited:
  - every `Tool::call` gets the token;
  - Bash kills its process group;
  - each aborted call gets a `tool_result` with `is_error: true` and the text
    "Interrupted by user".
- **Tools not yet started** get the same result without running.
- **Background shells** (`run_in_background`, or a Bash command that outlived
  its timeout, C22) survive an interrupt. Only KillShell, `/tasks stop` or
  session end stops them.
- **The transcript** records the partial assistant message, the interrupted
  tool results, and a synthetic user text "[Request interrupted by user]", so a
  resume shows exactly what happened.
- **In stream-json mode** the turn ends with a `result` (subtype `success`, plus
  `stop_reason: "interrupted"`). The process keeps reading stdin.

### C4. Rewind (file checkpoints)

- **What is captured:** content snapshots, not git. Before the first write to
  a file in each user turn, the edit tools save the file's prior content to
  `~/.kodact/file-history/<session-id>/<sha256(path)[..16]>@v<n>`. They do this
  for Write, Edit, MultiEdit and NotebookEdit, and for "file did not exist".
  It works in non-git directories and in `--add-dir` directories.
- **What is not captured:** writes made by Bash (`sed -i`, `git checkout`,
  build output) and writes made outside KodAct. The escape hatch for those is
  git, so `/rewind` warns when the working tree has changes it can't restore.
- **What rewind does:** rewinding to a user message restores every file that
  was checkpointed after that message to its snapshot, and deletes files that
  were created after it. `rewind_files` with `dry_run: true` lists the changes
  without applying them.
- **Files committed after a checkpoint** are still restored from the snapshot;
  rewind never touches git history.

### C5. Session storage

- **Location:** `~/.kodact/projects/<project-key>/<session-id>.jsonl`.
- **Project key:** the canonical cwd with every non-alphanumeric character
  replaced by `-` (`/home/u/app` becomes `-home-u-app`). That's readable and
  matches the reference CLI's project layout. Keys longer than 200
  characters are cut to 180 and given a `-<sha256[..12]>` suffix.
- **Index:** `~/.kodact/projects/index.json` maps each key to its cwd, so
  `/resume` can show real paths.
- **Format:** one JSON object per line. Each entry has `type`, `uuid`,
  `parentUuid`, `sessionId`, `timestamp`, `cwd`, `gitBranch` and `version`, plus
  either a `message` or a summary/title payload.
- **Writes:** entries are appended and flushed after every event, so a crash
  loses nothing. `--fork-session` copies the chain into a new id.
  `--no-session-persistence` writes nothing.
- **Key collisions:** two different directories can share a key (`/a-b` and
  `/a/b` both become `-a-b`). That's allowed:
  - every entry records the exact `cwd`, and the index maps a key to a *list*
    of cwds;
  - `--continue` and `/resume` filter by exact canonical cwd, never by key
    alone.

  Test: `session::c5_key_collision_keeps_projects_apart`.
- **Extra directories:** the session's `additionalDirectories` (from `--add-dir`
  and `/add-dir`) are stored in a `session_meta` entry each time they change.
  A resume restores them, and any `--add-dir` flags on the resuming command are
  merged in (union). Directories added mid-session apply from the next tool
  call on.
- **System events:** `model_fallback`, `compact_boundary`, `microcompact` and
  `permission_mode` changes are written as `system` entries. A resumed session
  shows why its model, context or mode changed.

### C6. Fallback model

`--fallback-model a,b` switches models in two cases:
- the primary returns **529 / overloaded_error** after its retries (3 by default)
  are used up;
- it returns **404 not_found_error** for the model.

The fallback applies to the rest of that user turn only; each new user turn
starts on the primary again. A switch emits a `system` message with subtype
`model_fallback` naming both models, and the same entry is written to the
session JSONL (C5). 429 doesn't trigger a fallback; it's retried with
`retry-after`.

### C7. Budgets and limits

- **`--max-turns N`** counts assistant API calls in a run. Reaching it ends
  with `error_max_turns`.
- **`--max-budget-usd X`** is checked after each API call. Pricing comes from
  the compiled model table in `kodact-api/src/models.rs`, which settings can
  override with `modelPricing`. Exceeding it ends with `error_max_budget_usd`.
- **Unknown models fail closed.** If the active model, or any
  `--fallback-model`, has no price in the table and no `modelPricing`
  override, `--max-budget-usd` refuses to start:
  - the error names the model and the setting to add;
  - in stream-json mode this is an `error_during_execution` result before any
    API call.

  Without a budget, an unknown model costs 0 and `/cost` says pricing is
  unknown. Test: `engine::c7_unknown_model_budget_fails_closed`.

### C8. Headless AskUserQuestion and ExitPlanMode

- **With an SDK host:** both tools go through `can_use_tool`. The host's
  `updatedInput` carries the answers / approval, the same way an IDE answers
  them.
- **With no host (`-p` alone):** AskUserQuestion returns an `is_error` result
  ("no interactive user; proceed with your best judgement and state your
  assumptions"), and ExitPlanMode is denied, which keeps the session in plan
  mode.
- **Always asked:** these tools (and EnterPlanMode) report `needs_user`, so
  they ask whatever allow rules or modes say. There are two exceptions:
  - deny rules deny them, and `dontAsk` denies them;
  - `bypassPermissions` approves them.
- **Mode changes:** a successful EnterPlanMode sets `plan`. A successful
  ExitPlanMode leaves `plan` for `default`, unless the approval's
  `updatedPermissions` already set a mode (e.g. `acceptEdits`).
- **Sub-agents** never get these tools.

### C9. Compaction

KodAct shrinks context in three ways.

**Micro-compaction** clears stale tool results.
- **What is eligible:** a `tool_result` that is all of the following:
  - from a tool whose output can be fetched again (Read, Grep, Glob, LS, Bash,
    BashOutput, WebFetch, MCP read tools);
  - older than the 3 most recent user turns;
  - larger than 8,000 characters.
- **When it runs:** when the next request's context (see "The size" below)
  exceeds 50% of the auto-compact threshold.
- **What it does:**
  - each eligible result's content is replaced with `[Old tool result cleared
    to save context. Re-run the tool if you need it again.]`;
  - `is_error` and the `tool_use_id` stay unchanged;
  - clearing is **sticky and append-only**: the cleared ids are written to the
    JSONL as a `system/microcompact` entry and applied on every later request.
    The cached prefix changes once, not on every turn.
- **What it doesn't change:** the transcript keeps the original content.
  Micro-compaction only rewrites what is sent to the API.

**Auto-compact** summarizes the conversation.
- **The window** is `--autocompact <tokens>` (100k–1M), or `autoCompactWindow`
  in settings, or the model's context window.
- **When it runs:** when the next request's context reaches
  `window − min(max_output, 32,000) − 13,000`.
- **The size** of the next request is the last reply's reported input, plus
  what the next request carries of that reply, plus an estimate for the
  messages added since. The reply's reasoning counts only when the provider
  sends reasoning back (the Messages API, Gemini). Chat completions leave it
  out, so there it counts the reply's text and tool calls, not its output
  tokens: a reply that reasoned 32k tokens and wrote a short call adds the
  call. Summary requests and side questions count toward cost, not toward
  the conversation's size.
- **What it does:**
  1. runs `PreCompact` hooks with `trigger: "auto"`;
  2. asks the model for a structured summary, with no tools and a reply's
     whole output budget (a model that reasons spends it before the summary);
  3. starts a new message chain: a compact-boundary `system` entry, then a user
     message holding the person's requests and the summary, then the most recent
     user turn if one is still pending.

**The person's requests go with every summary, word for word**, ahead of it,
whatever the summary kept: a summary that kept only the last step once left a
model rebuilding its task from that step. They fill at most a tenth of the
model's window (at about four characters a token, at least 2,000 characters):
the first request, which usually states the task, then the latest ones, and a
request too long for that keeps its beginning. A large window keeps them all
whole. They are recorded as `system/requests`, so the next compaction and a
resumed session carry them on; `/clear` forgets them. The summary prompt asks
the model not to copy them.

**`/compact [instructions]`** does the same with `trigger: "manual"` and
passes the instructions to the summary prompt. It doesn't micro-compact first.

**Both** emit `system/compact_boundary` with `{trigger, pre_tokens}`. The old
messages stay in the JSONL ahead of the boundary, so `--resume` loads only
what follows it.

**When the summary fails:**
- Every failed attempt emits `system/compact_failed` with `{trigger, reason}`,
  and for a reply that came back without a summary, its `stop_reason`,
  `output_tokens` and `reasoning_only` (the reply had reasoning but no text).
  The attempt isn't part of the conversation, so this is the only record of
  why it failed.
- A summary that came back empty is never sent again unchanged. It is tried a
  second time at once, with a note that the first attempt returned no
  summary. If that fails too, KodAct compacts without the model: the summary is
  replaced by a note of what it knows itself (the files changed this session,
  the task list, the last thing the model wrote), with
  `system/compact_fallback`. The person's requests come with it, as with every
  summary.
- A summary asks for the lowest reasoning level (`low`) wherever a level is sent, and always
  to an OpenAI-compatible endpoint: summarizing doesn't need depth, and at full depth a
  reasoning model could spend the whole budget thinking and return no summary.
- Auto-compaction does this for an empty summary. An API error leaves the
  conversation unchanged, and the next request tries again.
- When a request fails because the conversation no longer fits, any failure
  but an interruption gets the second try and then the note, since the run
  can't go on without compacting.

**Every request asks for no more reply than its window holds.** Endpoints that
count the requested output against the window (vLLM, LiteLLM and gateways in
front of them) refuse a request whose prompt and reply cap add up to more than
the window, even when the prompt alone fits. So each request, summary calls
included, asks for the output cap or the room the model's window has left after
the prompt, whichever is smaller, less a margin for the estimated part of the
prompt (a fiftieth of the window, at least 1,024 tokens), and never less than
4,096 (or the cap): with less room than that, a refusal compacts and retries.
The window is the model's own (its limits, `modelLimits`,
`KODACT_CONTEXT_WINDOW`, what the endpoint reports), never the `--autocompact`
window, which may be lower on purpose. On a large window, or early in a
conversation, the cap goes out unchanged.

Tests: `compact::c9_micro_clears_only_eligible_results`,
`compact::c9_micro_is_sticky_across_requests`,
`compact::c9_auto_triggers_at_threshold`,
`engine::reasoning_the_provider_does_not_send_back_is_not_counted_as_context`,
`engine::a_summary_request_does_not_change_the_measured_context`,
`engine::a_summary_gets_a_whole_replys_output_budget`,
`engine::an_empty_automatic_summary_is_not_sent_again_before_every_request`,
`engine::the_note_without_the_model_keeps_the_first_and_latest_requests`,
`engine::the_persons_requests_survive_every_compaction_word_for_word`,
`engine::a_reply_budget_fits_what_the_window_has_left`,
`engine::requests_ask_for_no_more_reply_than_the_window_holds`,
`engine::the_requests_kept_scale_with_the_models_window`,
`compact::tests::requests_keep_the_first_and_the_latest_within_the_budget`,
`engine::an_empty_summary_is_recorded_and_retried_when_the_conversation_no_longer_fits`,
`openai::resends_reasoning_says_what_the_request_carries`.

### C10. Worktrees

`-w/--worktree [name]` creates a worktree:
- **Location:** `<repo>/.kodact/worktrees/<name>`. The default name is
  `session-<short id>`.
- **Branch:** `kodact/<name>`, started from `HEAD`.

The session runs with the worktree as its cwd, so it's stored under the
worktree's project key (C5). Its `session_meta` entry records
`worktree: {name, path, mainRepo}`, and the index entry carries `mainRepo`.
`/resume` run from the main repo lists sessions from the repo and from all of
its worktrees, labelled. Resuming a worktree session `cd`s into the worktree.

Exiting a worktree session:
- **TUI:** asks whether to keep or remove the worktree.
- **`-p`:** leaves it in place.

A worktree with uncommitted changes is never removed automatically. Test:
`git::c10_worktree_session_listed_from_main_repo`.

### C11. Hook exit codes

**Exit code 0** means success.
- If stdout is a JSON object, it's parsed as hook output.
- For `UserPromptSubmit` and `SessionStart`, plain stdout is added to the
  model's context. For every other event, stdout goes to the debug log only.

**Exit code 2** means block. What that does depends on the event:

| Event | Effect of exit code 2 |
| --- | --- |
| PreToolUse | The tool call is blocked. stderr goes to the model as the `tool_result` error. |
| PostToolUse, PostToolUseFailure | The tool already ran. stderr goes to the model next to the result. |
| UserPromptSubmit | The prompt is blocked and erased. stderr is shown to the user only. |
| Stop, SubagentStop | Stopping is blocked. stderr goes to the model, which keeps working. |
| PreCompact, Notification, SessionStart, SessionEnd | Nothing is blocked. stderr is shown to the user. |

**Any other non-zero code** is a non-blocking error: stderr is shown to the
user and execution continues.

**Time limit:** commands time out after 60 s (`timeout` per hook overrides
this). A timeout counts as a non-blocking error.

**JSON output** may contain:
- `continue: false` (with `stopReason`), which stops the whole run;
- `suppressOutput`, `systemMessage`;
- `decision: "block"` with `reason`, which is equivalent to exit code 2 for the
  event;
- `hookSpecificOutput`, which can hold:
  - `permissionDecision` (`allow`, `deny` or `ask`) with
    `permissionDecisionReason`, PreToolUse only. `allow` skips the prompt but
    not deny rules;
  - `updatedInput`, PreToolUse only;
  - `additionalContext`, for UserPromptSubmit, SessionStart and PostToolUse.

Tests: `hooks::c11_exit2_semantics_per_event` and
`hooks::c11_json_permission_decision`.

### C12. Verification before finishing

A main-agent turn that changed files doesn't end on the model's first
"done" unless a check ran after the last change.

**What counts as a change:**
- a write by an edit tool (Write, Edit, MultiEdit, NotebookEdit), including
  writes made by sub-agents, taken from the file history;
- a shell command not classed as read-only, but only when the files'
  fingerprint differs from the one taken at the turn start or at the last
  check. The fingerprint covers the path, size and mtime of every tracked or
  untracked, not ignored file (caches like `__pycache__/` and `.pytest_cache/`
  skipped), plus the content of the files written this turn. It looks at the
  files, not at git's state, so `git add`, `git commit` or a new branch is not
  a change. Outside git only the files written this turn are followed, so a
  shell edit of one after its last check (`sed -i … fix.py`, then nothing run)
  is still seen. A command that rewrites a file and runs it (`cat > f <<EOF …
  EOF` then `./prog f`) is a check.

**What counts as a check:** a Bash call whose command (after env assignments,
wrappers like `timeout`, `uv run`, and shell keywords like `do`, `if`) starts
with:
- a configured or detected check command;
- a well-known build, test or lint tool;
- or runs a `build*`/`test*`/`check*`/`eval*`/`bench*` script, a test file, or
  one of the files changed this turn;
- or runs a program on a changed file (`./sim -f input.cfg`,
  `shellcheck x.sh`, `./app --config=config.json`, `./prog < input.txt`);
- or runs inline code (`python3 -c`, `node -e`, `python3 - <<EOF`) that names
  a changed file, imports a changed module, or names it in a string
  (`importlib.import_module('optimizer')`, `require('./util')`).

Not checks, even when they name a changed file: viewers, file managers,
editors, version control, archives, formatters and installers (`cat f`,
`cp f g`, `git add f`, `black f.py`, `pip install -r f`), in-place edits
(`sed -i`, `perl -pi -e`), redirect targets (`echo x > f`), and the lines of a
here-document (`cat > run.sh <<EOF … EOF` is a file being written).
`bash -c "…"`, and a here-document fed to a shell (`bash <<'EOF' … EOF`), are
judged on the commands they run. A check that only reaches the
changed file through another file (a harness that loads it) isn't visible on
the command line and doesn't count.

A check that fails still counts as evidence.

**Plain data read back is checked.** An answer, a table or a log that no
program runs (`.txt`, `.csv`, `.tsv`, `.md`, `.log`, `.out`) counts as
checked once it is read back after its last change: with Read, or a viewer
or a comparison in Bash (`cat`, `head`, `tail`, `od`, `xxd`, `diff`, `cmp`
and the like). Code and configuration still need a run of what uses them. A
package's `__init__.py` counts as imported when inline code names the package.

**The reminder:** when the model stops with unchecked changes, a meta user
message names the changed files and the project's checks, asks for them to
run, and asks the final answer to say what was and wasn't verified. When
nothing changed after a failed check, a different reminder asks the model to
fix it or say plainly that it fails. Each reminder:
- is recorded and emitted as `system/verification` with
  `{kind, files, shell}` or `{kind, command}`;
- comes before the Stop hook;
- is limited to `verification.maxReminders` per turn (default 1, 2 when
  unattended);
- comes once per set of changes: after one, another comes only when a file
  was written, or the worktree changed through the shell, since.

**No reminder when:**
- the stop reason is `refusal`;
- the turn is in a sub-agent;
- Bash isn't available;
- rules deny the check command.

Tests: `engine::verify_reminds_once_when_changes_are_unchecked`,
`engine::verify_accepts_a_check_after_the_last_change`,
`engine::verify_sees_shell_writes_through_git_and_failed_checks`,
`engine::verify_ignores_a_commit_after_a_passing_check`,
`engine::verify_sees_a_shell_rewrite_after_the_last_check_outside_git`,
`verify::recognizes_a_program_run_on_a_changed_file`,
`verify::recognizes_inline_code_that_names_a_changed_module`,
`verify::eval_and_bench_scripts_are_checks`,
`verify::here_document_bodies_are_data_not_commands`,
`verify::a_package_is_imported_by_its_name`,
`verify::files_read_back_are_named_and_plain_data_is_told_apart`,
`engine::an_answer_file_read_back_is_checked_but_code_is_not`,
`engine::a_reminder_comes_once_per_set_of_changes`.

### C13. Recovery

**Edit errors** (`kodact-tools`): when `old_string` isn't found, the error
adds, in this order of preference:
1. `old_string` contains Read's line-number prefixes;
2. the lines match except for whitespace (shown numbered);
3. the most similar window of the same length, at least 60% similar (shown
   numbered);
4. "nothing similar".

When there are several matches, the error lists the line each one starts on.

**Loop guard** (`kodact-engine/stuck.rs`): within one user turn, each pattern
below fires at most once. When it fires, a meta user message follows the tool
results, and `system/loop_guard` `{kind, ...}` is recorded and emitted. The
patterns:
- the same tool call (name and input) failing 3 times with the same error. A
  command that fails differently after each fix is making progress and isn't
  counted. Test runners' and build tools' own timings (`in 0.12s`,
  `finished in 0.00s`) and Python object addresses are ignored when comparing,
  but a program's own timing is not. When files changed between those
  failures, the reminder says the changes didn't affect the failure and asks
  the model to check its assumption about the cause (`after_changes: true`);
- the same call returning the same result 3 times with nothing changed in
  between (TodoWrite excluded). A check or build run again after an edit,
  with the same clean output, is a check, not a repeat. Read-only calls
  (reads, `cat`, BashOutput polls) count as before. A change is a successful
  Edit, MultiEdit, Write, NotebookEdit or Task call, or a shell command that
  isn't read-only;
- an Edit or MultiEdit replacement that reverses an earlier one on the same
  file;
- 5 failed calls in a row.

Denied calls are not counted. What the model can no longer see is forgotten:
a compaction, a rewind or `/clear` resets the guard, and a microcompaction
clears the same-result counts, so re-reading after one isn't called a loop.

**Truncated output:**
- A tool input that isn't valid JSON when its block stops (the `max_tokens`
  case) is kept, as `{"_truncated_input": ...}`, instead of failing the
  stream, with its raw text (`_raw_input`, the last 16 KB).
- The stop reason tells why. Stopped by `max_tokens`: the call is answered
  with an error explaining how to split the work. Ended by the model: the
  arguments were written invalid. A small, safe repair (a missing comma
  between fields, a missing closing quote or bracket, raw control characters
  in a string, stray closers after the object) is applied and the call runs,
  with `system/tool_input_repaired`. Otherwise the call is answered with an
  error saying the JSON was invalid and showing how it ended. Repairs are never
  applied to a call cut off by `max_tokens`: its content is incomplete.
- A text reply stopped by `max_tokens` is continued with a meta prompt, up
  to 3 times. The pieces join into one result.
- A reply stopped by `max_tokens` that was all reasoning (no text, no tool
  call) is not asked to continue: that only starts another reasoning pass up
  to the same limit, and on a hard task it repeats until the run ends with
  nothing done. It is asked instead to act on what it has (take a step, or
  answer), and `system/act_reminder` is emitted. Up to 3 in a row: a tool call
  resets the count, so a long task gets one each time it stalls after real
  work, while three stalls in a row with no tool call let the turn end. These
  don't use up the 3 text continuations.
- A turn that ends that way, with no answer (its last reply had no text and
  no tool call, and the reminders to act are used up), ends as an error, not
  a success: `error_during_execution`, `is_error: true`, no `result`, and
  `errors` holding "The model stopped without an answer: …". In print mode the
  exit code is 1, so a script doesn't take an empty run for finished work.
- A reply that ends normally with no text and no tool call (only reasoning, or
  nothing) doesn't end the turn either: it answers nothing. After the other
  end-of-turn reminders and before the Stop hook, it gets the same request to
  act or answer (`system/act_reminder` with `kind: empty_reply`; the
  cut-off's is `kind: reasoning_cutoff`), from the same budget of 3 in a row
  without a tool call.
- Where the provider doesn't send reasoning back (chat completions, except to
  Gemini), the model no longer has that reply's reasoning in the next request,
  so "act on what you have" would ask for what it lost. There the reminder
  carries the reasoning: whole up to 8,000 characters, otherwise its first
  2,000 and its last 6,000 (how it began and where it stopped). The event's
  `reasoning_chars` says how much it carried (0 where reasoning is sent back).
- After the first `max_tokens` stop, the turn's output cap rises to 64,000
  (bounded by the model), unless the user set `KODACT_MAX_OUTPUT_TOKENS`, and
  `system/output_limit` is emitted.

Tests:
- `builtin::edit_failures_point_at_the_nearest_text`;
- `stuck::*`;
- `engine::loop_guard_reminds_after_repeated_failures`;
- `engine::loop_guard_forgets_results_the_model_lost_to_compaction`;
- `engine::loop_guard_tells_when_changes_did_not_affect_a_failure`;
- `engine::max_tokens_continues_text_and_answers_cut_off_calls`;
- `engine::a_reply_cut_off_while_reasoning_is_asked_to_act`;
- `engine::an_empty_reply_is_asked_to_act_instead_of_ending_the_turn`;
- `engine::a_reply_that_was_all_reasoning_gets_it_back_where_it_is_not_kept`;
- `accumulate::invalid_tool_json_is_marked_truncated`.

### C14. Dangerous commands and injected instructions

**Threat patterns** (`kodact-permissions/threat.rs`):
- Every Bash command is scanned.
- A high- or critical-risk match makes `decide` return `Ask`, with
  `Reason::Threat` and no suggestions (`Deny` under `dontAsk`). This happens
  after deny rules and before ask rules, allow rules, mode defaults and the
  sandbox. `bypassPermissions` is exempt.
- Medium-risk matches (history tampering) are reported by `scan_command`
  but not escalated.

**Injection markers** (`kodact-tools/injection.rs`):
- After a successful call, and after the PostToolUse hooks, the output of
  every tool except the edit tools and TodoWrite is checked for:
  - phrases aimed at an agent;
  - chat-template tokens;
  - system tags;
  - Unicode tag characters.
- On a match, a note quoting the match is appended to the tool result.
  Known false positive: reading an agent harness's own source, where those
  tags appear as string literals.

Tests:
- `threat::flags_dangerous_commands`;
- `threat::leaves_ordinary_commands_alone`;
- `permissions::flagged_commands_are_never_approved_automatically`;
- `injection::*`;
- `engine::injected_instructions_in_tool_output_are_marked`.

### C15. Context budgets and the plan

**Output budgets:**
- Bash keeps 30,000 characters of stdout and of stderr (`max_output_chars`).
- Any other tool except Read keeps 50,000 characters of text.
- Longer output keeps its head and tail. The full text is written to
  `<cache>/tool-output/<session>/<tool_use_id>-<label>.txt`, and the result
  names that file.
- The permission engine lists that directory in `read_dirs`, so Read and
  Grep on it need no prompt (writing there still asks).

**Repeated reads:**
- `FileState` keeps, per `(path, offset, limit)`, the content hash and the
  tool_use_id of the Read that showed it.
- The same range of the same content returns a short "unchanged" note
  (structured `{"type": "file_unchanged"}`).
- Micro-compaction forgets the views whose results it cleared; full
  compaction forgets all of them.
- Sub-agents have their own `FileState`.

**The plan:**
- After compaction, the TodoWrite list is added to the summary message and
  recorded as `system/todos`.
- `restore` rebuilds the list from the last TodoWrite call after the
  boundary, else from that record.

**Budget reminder:** with `--max-turns` of at least 6, or `--max-budget-usd`,
one reminder per turn when 3 calls (or 15% of the budget) are left. With
`--max-time`, the first prompt states the time left, and the same reminder
comes when a fifth of the time (at least a minute) is left. The reminder asks
first to put back a better earlier version if the one in place tested worse,
then to finish and report. The limit is counted from the start of the
session; the engine never stops on it.

Tests:
- `builtin::long_output_is_saved_in_full_and_pointed_to`;
- `permissions::read_dirs_are_readable_not_writable`;
- `builtin::repeated_reads_of_unchanged_content_are_not_resent`;
- `engine::the_plan_survives_compaction_and_resume`;
- `engine::the_model_is_told_when_turns_run_low`.

### C16. MCP servers and trust

**Sources**, later ones replacing earlier ones by name:
1. `.mcp.json`;
2. `mcpServers` in settings;
3. `--mcp-config`.

`--strict-mcp-config` and `--bare` read only `--mcp-config`.

**Trust:** a `.mcp.json` server is a command shipped with the repository.
- It starts only when the user's own settings trust it: the user or local
  layer, via `enableAllProjectMcpServers` or `enabledMcpjsonServers`
  (`kodact mcp approve`).
- `disabledMcpjsonServers` (user or local layer) wins over both, and also
  turns off servers from `mcpServers` in settings. `--mcp-config` and plugin
  servers are chosen per run and aren't affected.
- The project's checked-in settings cannot approve it.
- An unapproved server shows as `disabled` in `system/init`, with a warning
  on stderr.

**The same rule for the endpoint:** `baseUrl`, `openai.baseUrl`,
`apiKeyHelper` and `openai.apiKeyHelper` in the project's checked-in settings
are ignored with a warning (`kodact-core/src/endpoint.rs`). Otherwise cloning a
repository could send the user's key to another host, or run a command at
startup without approval.

**Startup:**
- All servers connect concurrently before the first turn: `initialize`,
  then `notifications/initialized`, then `tools/list` (paged).
- Each has `MCP_TIMEOUT` (default 30 s) to connect. Calls have
  `MCP_TOOL_TIMEOUT` (default 10 min).
- A failing server is reported, with its stderr tail, and never fails the
  session.

**Tools:**
- Server tools are named `mcp__<server>__<tool>`: cleaned to
  `[A-Za-z0-9_-]`, at most 64 characters, with a hash suffix when cut.
- They join after `--tools` filtering and before `--disallowedTools`.
- They always go through permissions, since `readOnlyHint` is trusted only
  for running calls in parallel.
- Their output passes through the output budget (C15) and the injection
  markers (C14).

**Server requests:** the client answers `ping` and `roots/list` (the project
directory). Other server requests (`sampling`, `elicitation`) get "method
not found".

**`/import`** follows the same trust rule:
- servers from the user's own configs (`~/.codex`, `~/.gemini`,
  `~/.cursor`) go to user settings;
- servers shipped in the repository (`.cursor/mcp.json`) go to `.mcp.json`
  and still need approval.

**Turning servers off and on** (`/mcp reconnect|enable|disable <server|all>`,
and the stream-json `mcp_reconnect` / `mcp_toggle` requests):
- Each server's live state (status, client, tool and prompt lists) sits
  behind its own lock, shared with the tool objects made from it. Disabling
  stops the server and hides its tools, prompts, resources and instructions
  at once, in the session and in sub-agents. Enabling or reconnecting starts
  it from the config it was resolved with; the same tool objects work again.
- A tool the server no longer lists is hidden at once. A tool name it didn't
  list when the session was built has no tool object until the session is
  rebuilt (`/reload-plugins`, `/clear`, a new run).
- `disable` saves the name in `disabledMcpjsonServers` in the launch
  project's `.kodact/settings.local.json`; `enable` takes it out. A server
  waiting for approval stays off: `enable` never grants trust.
- A restart stops the old process before starting the new one, so calls in
  flight to that server fail. Changes to one server run one at a time.

**`kodact mcp serve`** exposes the built-in tools without prompts, since the
client approves, but refuses commands matching dangerous-command patterns.

### C17. Command dispatch and surfaces

**One registry.** `kodact_core::commands::BUILTINS` is the only list of
built-in commands: name, aliases, argument hint, description, the surfaces
it runs on, and whether it is immediate. `/help`, `system/init`
`slash_commands`, the `initialize` response's `commands` and the TUI menu
are all generated from it.

**Resolution**, only at the start of a user message:
1. a built-in, by name or alias;
2. a custom command;
3. a skill, or a chain of up to six (`/a /b text`);
4. an MCP prompt (`/mcp__server__prompt`).

A name containing `/` or `\`, a name with a `.` (a file name), or one that
is an existing root path, is an ordinary prompt when no command has that name. Anything else is `Unknown command: /name`. Commands the
reference ties to its vendor's account or cloud are not registered, so they
get the same answer.

**One driver.** `kodact_core::Driver` owns the engine and the catalog. Every
front end (print, stream-json, REPL, TUI) passes each input to
`Driver::input`, which returns either a `TurnResult` or `Exit`.
- A command either expands to a prompt for the model, or answers locally.
- A local answer is a `TurnResult` with `num_turns: 0`, no stop reason and
  `is_error` on failure, so every output format and exit status works
  unchanged.
- Commands need no front-end code. A command that needs a choice takes
  its answer as arguments, so it works in `-p`; the TUI draws a picker
  whose rows are those arguments (`commands::picker`).

**Changing the running session.** Settings commands change the live
objects and never rebuild the session:
- the engine handle's runtime (model, effort, thinking, fast mode, permission
  mode);
- the engine config (compaction);
- the tool context's sandbox cell, which sub-agents share;
- the permission engine's rules and directories;
- the system prompt, rebuilt from `PromptSpec`.

`PromptSpec` keeps every input of the system prompt: the replacement or
appended text, the agent prompt, MCP instructions, the output style and the
environment. That way `/output-style`, `/model` and an SDK host's
`initialize` rebuild it without dropping a part. A note for the model (a new
working directory) rides on the next prompt as a system reminder, so the
cached prefix stays intact.

**Saving.** On interactive surfaces, settings commands also write the
default to the same layer the reference uses. In `-p` and stream-json they
don't. A write is mirrored into the loaded layers, and the answer names any
higher-precedence layer that overrides it.

**Switching sessions.** `/clear`, `/resume`, `/branch`, `/cd` and `/reload-*`
replace the session through `Driver::switch`. It rebuilds the session with
`build_session` from the launch options the front end handed over
(`set_rebuild`). Several things carry over:
- the MCP manager, so connections stay up;
- the running model, effort, thinking, fast mode and permission mode;
- the output style and an SDK host's system prompt;
- the cost and usage so far.

`/branch`, `/cd` and `/reload-*` take the conversation from memory
(`Resume::Loaded`, a snapshot of the live state), so they work with
`--no-session-persistence`. The old session gets SessionEnd (except on a
reload) and the new one SessionStart with `source` set to `clear` or
`resume`.

Front ends hold a `Live` handle, never an `EngineHandle`: Ctrl-C and SDK
control requests (`interrupt`, `set_model`, `rewind_files`, ...) always
reach the current engine. After a switch, stream-json hosts get a new
`system/init` with the new session id.

**Rewind.** `/rewind` works on the person's prompts (user text that isn't a
tool result or only a system reminder):
- **Conversation:** truncates the messages and branches the transcript from
  the message before, with a `system/rewind` record so a resume sees the
  same thing.
- **Code:** the file checkpoints of that turn (C4).
- **Partial summaries:** replace a range with a summary note. For
  `summarize-to`, the kept messages are written again after a compact
  boundary under their original uuids, so checkpoints and SDK `rewind_files`
  ids stay valid.

**Tools that come and go.** Some tools are registered once but shown only
while a setting allows them: the Advisor (while `/advisor` names a model)
and later disabled MCP servers. `ToolRegistry` filters on `is_enabled()`
for the tool list *and* for lookups, so a hidden tool can't run even if
the model names it. Showing or hiding one changes the tool list, which
costs one cache miss on the next request.

**Immediate commands** (`/status`, `/usage` with `/cost` and `/stats`,
`/tasks`, `/context`, `/mcp`, `/btw`, and the TUI's `/keybindings` and
`/terminal-setup`) are marked `immediate` in the registry and answer at once,
even while a turn is in progress. `commands::immediate(text, &catalog)`
decides: an immediate built-in, aliases included. Arguments don't change it
(`/mcp reconnect x`, `/tasks stop x` and `/context all` are immediate too),
and built-ins win over custom commands, so the catalog never turns one into
something else.

*The session view* (`kodact_core::view::SessionView`). A turn holds the
driver mutably, so these commands never touch it. They read a view that is
cheap to clone (an `Arc` inside) and survives session switches, as `Live`
does. It has two parts, each an `Arc` replaced whole on every update:
- **The driver's** (`ViewState`): session id, cwd, init facts, settings and
  warnings, surface; the engine handle (runtime, permissions); the
  transcript (title, schedule record); the tool context (working
  directories, sandbox policy, background shells); the provider; a hooks
  summary; start time and activity counters; `/btw` exchanges; skill names;
  memory files; subtask rows (live `Arc`s: running state, tool calls, stop
  token); finished subtasks; the shared scheduler; the MCP manager. The
  driver publishes it when idle (`Driver::sync_view`, at the start and end of
  every input and scheduled task, after a switch) and after each turn of an
  input (`record`), so a goal's later turns see the earlier ones.
- **The engine's** (`kodact_engine::EngineSnapshot`, behind
  `EngineHandle::snapshot()`): the conversation (`Arc<Vec<Message>>`),
  cleared tool results, system blocks, tool specs, cost, usage, per-model
  usage, context tokens, the configuration `/context` and `/btw` need, and
  the turn in progress (`TurnProgress`: model calls, API time, tool calls so
  far). The engine publishes it after each model call, after each tool batch
  and at the end of a turn; the driver also asks for it when idle. Each
  publish copies the conversation once; readers share it. Sub-agent engines
  don't publish (nothing reads them).

Readers clone the inner `Arc` and drop the lock at once: no guard is held
for long, and none across `.await` (clippy's `await_holding_lock`, on by
default in its `suspicious` group, checks it).

*One implementation.* `/status`, `/usage`, `/tasks`, `/context`, `/mcp` and
`/btw` live in `commands/immediate.rs` and take `&SessionView`. The idle
path (`commands::execute`) syncs the view, runs the same function and syncs
again, so idle output is what it always was. Mid-turn, `/usage` adds the
turn in progress to the totals.

*Deferred effects.* What needs the driver is recorded on the view as an
`Effect` and applied by `sync_view` when the driver is next free (the end of
the turn, or at once when idle; front ends' idle loops wake on
`effect_recorded()`):
- `RefreshMcp`: after `/mcp reconnect|enable|disable`, the servers'
  instructions and the `system/init` facts. The manager itself changes at
  once: it serializes each server's changes, and tool calls see a
  consistent state;
- `SideUsage`: `/btw`'s cost, priced by the engine (`record_side_usage`);
- `SideQuestion`: the `/btw` exchange, kept for the next side question.
  Until it is applied, `SessionView::side_questions` adds it to the
  driver's, so a second `/btw` in the same turn sees the first.

*`/btw` mid-turn* builds its request from the snapshot
(`kodact_engine::side_question_request`, shared with the engine's own
requests): same system prompt and tools, `tool_choice: none`. A model reply
whose tool calls have no results yet is left out. It uses the view's
provider and a token of its own, so Esc interrupts the turn, not the side
question. When idle, Ctrl-C cancels it as before.

*Per surface:*
- **TUI:** `App::submit` sends an immediate command at once while busy
  (echoed, not queued). The session task drives `driver.input(..)` (and a
  scheduled task's turn) as a pinned future in `select!` with `rx.recv()`:
  an immediate input is answered from the view on a task of its own
  (`UiEvent::Reply`), anything else waits in a local queue for the turn to
  end. The spinner keeps running.
- **Stream-json:** the stdin reader (`host::read_stdin`) answers an
  immediate command in a `user` message (a single text block) at once, with
  a `result` line in the usual shape (`num_turns: 0`, the current session
  id) plus `"immediate": true`, and never forwards it to the turn loop. A
  host tells it from the running turn's result by that field (the
  reference's public docs don't describe mid-turn commands). Idle or not,
  the reader answers them, so they never queue behind a prompt.
  `mcp_status`, `mcp_reconnect` and `mcp_toggle` control requests don't wait
  either.
- **Line REPL:** reads stdin only between turns, so commands typed during a
  turn run after it, as before.
- **`-p` with one prompt:** unchanged (there is nothing to run beside).

### C18. Goals

`/goal <condition>` sets a condition (at most 4,000 characters) and sends it
as the prompt. After every model turn while the goal is active:

1. The session's model (`goalCheckModel` picks another) checks the
   conversation against the condition. It sees the prompts, replies, tool
   calls and the end of each tool result, each labelled with its tool (the
   newest 60,000 characters), after a list of every failed tool call in the
   session (errors, and sub-agent results that start with `FAILED`). Its
   instructions: the agent's messages are claims, not evidence; failed work
   counts as not done unless redone; details no tool result shows are
   unverified. It answers `{"verdict": "met" | "not_met" | "impossible",
   "evidence": [...], "reason": ...}`. A `met` with no evidence, anything it
   can't parse, and text without JSON all count as `not_met`.
2. **met:** the goal is achieved. **impossible:** it fails, with the reason.
3. **not_met:** the driver starts another turn with a reminder that names
   the reason and the condition.

**Guards.** The loop stops but keeps the goal (paused) when:
- three turns in a row used no tool;
- a turn was interrupted, or a hook blocked the prompt;
- the check itself failed;
- `--max-turns` (counted across the loop) or the budget ran out;
- a transient API error occurred.

The next prompt that reaches the model resumes it; local commands don't.
An error that won't go away by itself (bad credentials, billing, a missing
model, a conversation too long even after compaction; `TurnResult.fatal`)
clears the goal.

**Ending it.** `/goal clear` (or `stop`, `off`, `reset`, `none`, `cancel`)
and `/clear` end it. `/goal` alone shows the status, the checks so far, the
time and the cost.

**Persistence.** Each change is a top-level `goal` record in the transcript,
outside the message chain like the title, so compaction can't drop it. A
resumed session gets back a goal that was still active.

**Output.** Every turn's `result` goes to the host as it finishes, followed
by a `system/goal` event (`status`: `active`, `achieved`, `failed` or
`cleared`, plus the reason). With a single `-p` prompt, the run exits with
status 1 when the goal ends unmet. In text mode, failures and pauses are
warnings on stderr; an achieved goal is reported only with `--verbose`.

`/goal` is refused while `disableAllHooks` is set, as in the reference,
where goals run as an end-of-turn check.

### C19. Scheduled prompts

A session can run prompts later: on a cron schedule (`CronCreate`, or
`/loop` with an interval) or self-paced (`ScheduleWakeup`, or `/loop`
without one). The scheduler is part of the session; `KODACT_DISABLE_CRON=1`
turns it off.

**Tasks:**
- At most 50; each has an 8-character id.
- Cron uses 5 fields in local time: `*`, values, ranges, steps, lists, and
  month and weekday names. When both day of month and day of week are set,
  either one matches.
- Tasks fire only while the session is idle, between turns. A task that
  came due during a turn fires once afterwards, with no catch-up.
- **Jitter**, the same every time for a task id:
  - recurring tasks run up to 30 minutes late (at most half their interval);
  - one-shots set for :00 or :30 run up to 90 seconds early.
- Recurring tasks expire seven days after creation: they fire one last time,
  then are removed.
- Every change writes a top-level `schedule` record. A resumed session gets
  its recurring tasks back; one-shots and wakeups don't survive.

**`/loop [interval] [prompt]`**, parsed in Rust, not by the model:
- The interval is a leading `5m`, `2h`, `1d` or `30s`, or a trailing
  `every 20m` / `every 5 minutes`. `check every PR` has no interval.
- Seconds round up to minutes. An interval cron can't step evenly (`7m`,
  `90m`, `5h`) is rounded to the nearest one that divides its unit, and the
  confirmation says so.
- **With an interval:** a recurring cron task, and the prompt also runs now.
- **Without one:** the prompt runs now with a note asking the model to call
  `ScheduleWakeup` (60-3600 s, with the `/loop` input as the prompt), or
  `ScheduleWakeup {stop: true}` when the work is done. One wakeup is pending
  at a time.
  - If an iteration does neither, a fallback check comes 20 minutes later;
    a second miss ends the loop.
  - An interrupted iteration ends the loop.
- **Without a prompt:** `.kodact/loop.md`, else `<config>/loop.md` (up to
  25 KB), else KodAct's maintenance prompt.

**Fired prompts:** a custom command, or skills the model may use, expand as
usual. Built-in commands, user-only skills and MCP prompts reach the model as
plain text. Each firing emits `system/scheduled_task`; `/loop` with an
interval emits `system/scheduled`. `/tasks` lists scheduled tasks, and
`/tasks stop <id>` deletes one.

**Front ends.**
- The REPL and stream-json fire due tasks while waiting for input.
- `-p` keeps running after its prompt while tasks are pending. It stops
  when none are left, on Ctrl-C, at `--max-turns` (counted across the run),
  or when the budget is spent.
- `KODACT_TEST_TIME_SCALE` speeds up the scheduler's clock, for tests only.

### C20. Subtasks

`/subtask <task>` forks the conversation into a background sub-agent.

**The fork.**
- The same system prompt, tools (same order and text) and messages as the
  main conversation, then a note saying it is a background subtask, and the
  task. Its first request reads the main conversation's prompt cache.
- Task, AskUserQuestion, EnterPlanMode, ExitPlanMode, CronCreate, CronDelete
  and ScheduleWakeup stay listed but refuse to run.
- It never asks: any permission prompt is denied with a reason. It starts
  with a copy of the main conversation's rules and mode.
- It shares the session's provider, hooks (SubagentStop at the end),
  sandbox, working directories and file checkpoints. Its edits get their own
  checkpoint turn, placed after the prompts made before it started, even
  while a later prompt runs (`FileHistory::add_turn` / `snapshot_in`): a
  `/rewind` to an earlier prompt undoes them, one to a later prompt doesn't,
  and they never count as the running prompt's writes. Its
  transcript goes under the session's `agents/` directory, with a `forkOf`
  record.
- With `--max-budget-usd` it may spend what was left when it started;
  `--max-turns` caps its turn. At most 8 run at once. Background shells it
  started are killed when it ends.

**Handing back**, the next time the session is idle (a front end waiting for
input, or the start of the next input or scheduled task):
- its spend joins the session's;
- the person gets a notice with its report;
- hosts get `system/subtask` (`status` completed, interrupted or error;
  `result`, `num_tool_calls`, `duration_ms`, `total_cost_usd`,
  `transcript_path`);
- its report reaches the model with the next prompt.

Starting one emits `system/subtask` with `status: started`, and the model
hears of it with the next prompt.

**Stopping.** `/tasks` lists subtasks, running and (the last 20) handed back; `/tasks stop <id>` interrupts one,
which is handed back as interrupted. `/clear` and `/resume` stop running
subtasks and pass no report on (their spend still counts); `/branch`, `/cd`
and the reloads keep them. Session end gives them 3 s, then aborts them.

**Front ends.**
- REPL: a notice when one finishes while the prompt waits.
- stream-json: `system/subtask` as soon as one finishes while idle; no turn
  runs by itself.
- `-p`: keeps running while subtasks are out, then hands them all back and
  runs one more turn, whose answer is the run's result.

### C21. Unattended runs (`--autonomous`)

For runs nobody watches: benchmarks, CI, scripts. Turned on by `--autonomous`,
`KODACT_AUTONOMOUS=1` or the `autonomous` setting.
- **Tools:** AskUserQuestion, EnterPlanMode and ExitPlanMode are removed, since
  nobody answers questions or approves plans.
- **Prompt:** the system prompt gains a "Running unattended" section
  (`prompts/05-autonomous.md`):
  - pick a reasonable reading and go on, instead of asking;
  - make a real attempt before concluding something can't be done;
  - plan with TodoWrite and finish every step;
  - when improving something by trial, keep the best tested version in place
    (try new ones on a copy) and put it back if a change made things worse;
    note each attempt's result, and after three attempts in a row at one idea
    that don't beat the best, switch to a substantially different idea (going
    back to a dropped idea counts as another attempt at it);
  - check the work before finishing;
  - write large files in parts;
  - a message without a tool call ends the run (after waiting for commands
    still running in the background, C22): do an announced step at once;
  - install missing tools (`apt-get`, `pip`) instead of working around them;
  - a command still running at its Bash timeout goes on in the background:
    don't start it again or poll it with `sleep`; never pipe long commands
    through `tail`;
  - if declining for safety or policy reasons, say so briefly.
- **Attempt reminder:** a turn that would end without a single tool call gets
  one reminder to make a real attempt. A `system` event `attempt_reminder` is
  emitted for it. It isn't sent for a refusal stop reason, for sub-agents, or
  twice in one turn, and the model may still answer without tools.
- **Next-step reminder:** a reply without a tool call whose last line announces
  a step ("Let me check the logs.", "I'll write the encoder:") is sent back
  with a reminder to do it now (`system` event `next_step_reminder`).
  "Let me know..." is an ending, not a step.
- **Todo reminder:** a turn that would end while TodoWrite items are not
  completed gets a reminder listing them (`todo_reminder`).
- Each of the two fires at most twice a turn, not for sub-agents or a refusal
  stop reason; the model may still end afterwards.
- **Background reminder:** a turn that would end while commands started with
  `run_in_background` still run gets one reminder listing them: they are
  stopped when the session ends, right after an unattended turn, so a service
  the task needs must be started again detached (`nohup <command> > <log>
  2>&1 &`). `system` event `background_reminder` with the shell ids. Commands a
  Bash call moved to the background at its timeout aren't listed (the turn
  waits for those, C22). Once a turn, not for sub-agents or a refusal. The
  Bash tool's description says the same for every session.
- **Verification:** `verification.maxReminders` defaults to 2 instead of 1. A
  setting still wins.
- **API retries:** a model call that fails for a reason a retry can fix (HTTP
  408, 409, 429, 5xx, overloaded, a lost connection, a stream that broke
  mid-reply) is sent again instead of ending the run, after the provider's own
  quick retries. Waits start at 5 s, double, and are capped at a minute; a
  `retry-after` header is honoured. Up to 10 retries in a row, or with `--max-time` up to
  30 while a minute is left after the wait. Each retry writes a warning and a
  `system` event `api_retry` (`attempt`, `retry_delay_ms`, `error`). Errors a
  retry can't fix (401-404, billing) still end the run at once.

Tests:
- `engine::unattended_runs_attempt_before_giving_up`;
- `engine::unattended_runs_retry_failed_model_calls`, `engine::api_retries_stop_at_their_limits`;
- `engine::unattended_runs_finish_announced_steps_and_open_todos`, `engine::announced_next_steps_are_recognised`;
- `engine::an_unattended_run_is_told_its_background_commands_will_be_stopped`;
- `prompts::autonomous_section_is_added_only_when_asked`;
- `core::unattended_runs_get_a_second_verification_reminder`;
- `e2e::autonomous_runs_have_no_question_tools_and_attempt_first`.

### C22. Long commands

Builds, installs, test suites and data scripts often outlast a Bash call's
wait. They are not killed for it.
- **The wait:** Bash `timeout` (default 2 minutes, maximum 10) is how long the
  call waits. A command still running then moves to the background as a shell
  (`bash_N`), as if started with `run_in_background`. The call returns its ID
  and the output so far (structured: `backgroundTaskId`, `stdout`, `stderr`,
  `interrupted: false`), not an error. Its working directory is not carried over.
- **Interrupts** still kill the command's process group (C3), and so does a
  call that goes away mid-command (the session ends) unless the command moved
  to the background. `!command`, hooks and other commands KodAct runs itself
  keep their own timeouts, which kill.
- **Exit notices:** when a background shell exits (either kind), the model is
  told with the next tool results, or with the next prompt: a system reminder
  with its exit code, run time and last unread output (`system` event
  `background_exit`). Not for a shell stopped with KillShell or `/tasks stop`,
  nor for one whose end BashOutput already showed. Each shell is reported once.
- **Waiting at the end of a turn:** a turn that would end while commands moved
  there at their timeout are still running waits for them, up to 10 minutes
  and a minute short of `--max-time` (notice "Waiting for bash_N…", `system`
  event `background_wait`), then goes on with how they ended, or with a note
  that they still run. At most twice a turn; an interrupt ends the wait.
  Commands started with `run_in_background` (servers, watchers) are not waited
  for. Unattended, this keeps the run's end from stopping a build the model
  wanted; at the terminal, the answer comes with the result.
- **Verification (C12):** a check that moved to the background counts when it
  exits, passed or failed by its exit code; until then it is not a check.
- **Output:** each stream keeps its first and last 4 MB, foreground or
  background, and BashOutput returns only what wasn't read yet.

Tests:
- `builtin::bash_timeout_moves_the_command_to_the_background`,
  `builtin::a_moved_command_can_be_stopped_with_its_children`,
  `builtin::a_dropped_bash_call_stops_its_command`,
  `builtin::finished_bash_calls_are_not_listed_as_background_shells`,
  `builtin::reading_a_finished_shell_counts_as_being_told`,
  `builtin::a_background_shell_that_exits_is_reported`,
  `shells::captured_output_is_read_in_parts_and_notes_what_was_dropped`;
- `engine::the_turn_waits_for_commands_that_outlived_their_timeout`,
  `engine::a_check_that_finishes_in_the_background_counts`;
- eval task `evals/tasks/slow-build` (a 150 s build).

## Prompts

KodAct ships its own system prompt and tool descriptions in
`crates/kodact-engine/prompts/`. At runtime it can load a different set from
`KODACT_PROMPTS_DIR`: a directory of `system/*.md` and `tools/<ToolName>.md`
files, for example a local, uncommitted copy of a published prompt extraction.
`--system-prompt[-file]` and `--append-system-prompt[-file]` work as their help
text says.

The default prompt's "How to work" section (`02-working.md`) asks for a change
to be checked with the project's own tests, linter or build, and for the work
to finish once every requirement is met and checked. A result that already
meets its stated target (a time, a size, a margin) isn't reworked unless the
task asks for the best possible. A borderline pass on a measurement that
varies between runs is run again, and its margin is reported. Test:
`prompts::the_prompt_says_to_finish_once_the_target_is_met`.

It also asks for work to go into files as it goes (code and long text written
step by step, a large file in parts) rather than composed whole in the
reasoning, which may not be kept and is cut at the reply limit; and, when an
output format is given, for that output to hold exactly it, with diagnostics on
stderr or left out. After a reply cut off while all reasoning, the act reminder
asks for a draft in progress to be written to its file. Tests:
`prompts::the_prompt_says_to_work_in_files_and_keep_output_formats_exact`,
`engine::a_reply_that_was_all_reasoning_gets_it_back_where_it_is_not_kept`.

## Configuration names

- **Environment:** only `KODACT_*` variables are read, the API ones included:
  `KODACT_API_KEY`, `KODACT_AUTH_TOKEN`, `KODACT_BASE_URL`, `KODACT_MODEL`,
  `KODACT_CUSTOM_HEADERS`. Other tools' variables are ignored.
- **Settings files** (later wins):

  | Layer | Paths |
  | --- | --- |
  | user | `~/.kodact/settings.json` |
  | project | `.kodact/settings.json` |
  | local | `.kodact/settings.local.json` |
  | flag | `--settings` |
  | managed | `/etc/kodact/managed-settings.json` |
- **Resources** (agents, commands, skills, output styles) live in
  `~/.kodact/<kind>/` and `.kodact/<kind>/`. Project wins over user when two
  names collide.
- **Memory files:** `~/.kodact/KODACT.md`, then `KODACT.md` or `.kodact/KODACT.md` in
  each directory from the filesystem root down to the cwd, each followed by its
  `KODACT.local.md`.
- **`kodact --version`** prints `<version> (KodAct)`.

## Read limits

- **Text:** reads 2,000 lines by default and cuts lines at 2,000 characters.
  `offset` and `limit` select a range.
- **Images** (png, jpg, gif, webp): sent as image blocks, up to 5 MB. Larger
  images are rejected with a message.
- **PDFs:** sent as `document` blocks, up to 32 MB. A PDF over 10 pages needs a
  `pages` range of at most 20 pages, given as a page count, not the raw file.
- **Notebooks (`.ipynb`):** rendered cell by cell with their outputs.
- **`@` mentions** (`kodact_agents::attach`, attached by `Driver::input` to
  plain prompts and by custom commands): 50,000 characters per file
  (`truncate_middle`), at most 10 files and 200,000 characters per message,
  directory listings of at most 200 entries; files over 10 MB, images, PDFs
  and binaries (a NUL in the first 8 KB, or not UTF-8) get a one-line note to
  use Read instead. The permission check is Read's decision on the path,
  never a prompt: `ask` and `deny` both leave the file out. Attached files
  are marked read (`FileState::record_read`) before the turn, and forgotten
  again if a UserPromptSubmit hook blocks the prompt. A file's `</file>`,
  `</directory>` and `</system-reminder>` are escaped so it can't close its
  wrapper, and text the injection scan flags (as for Read output) gets a
  note that it comes from the file, not the user.
