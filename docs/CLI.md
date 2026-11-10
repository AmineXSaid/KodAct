# KodAct command-line contract

This document is a public interface. Changing documented syntax, exit
statuses, machine output or config precedence is a breaking change and needs
a note in `CHANGELOG.md`.

## Conventions

| Topic | Contract |
| --- | --- |
| Executable | `kodact` (and `kodact-eval` for evaluation) |
| Help and version | `kodact --help`, `kodact <command> --help`, `kodact help <command>`, `kodact --version`. They need no credentials, no network and no config, and print to stdout. |
| Option style | Descriptive long options, with short aliases only for frequent ones (`-p`, `-c`, `-r`, `-n`, `-q`, `-d`, `-v`, `-h`). `--` ends option parsing: `kodact -p -- "-v means?"` |
| Streams | **stdout** carries results only: the answer, a JSON document, an NDJSON stream, or `config` output. **stderr** carries diagnostics, warnings, progress and errors. |
| Machine output | `--output-format json` writes exactly one JSON document; `--output-format stream-json` writes NDJSON. Neither ever contains banners, colors or logs. |
| Color | Color is on when stdout and stderr are terminals and `TERM` isn't `dumb`. `NO_COLOR` (non-empty) turns it off, and `FORCE_COLOR` / `CLICOLOR_FORCE` turn it on. `--color auto\|always\|never` overrides all of these. Machine output is never colored. |
| Quiet and verbose | `-q/--quiet`: errors only on stderr. `--verbose`: adds informational diagnostics. Both affect stderr only, never stdout. `-d/--debug` and `--debug-file <path>` turn on debug logs. |
| No terminal UI | `--no-tui` (or `KODACT_TUI=0`): interactive mode uses the line-based prompt. |
| No input | `--no-input` (or `KODACT_NO_INPUT=1`): KodAct never waits for a person. Permission prompts are denied, and interactive mode refuses to start. |
| Confirmations | KodAct has no `--yes`. Tool permissions are granted only by `--permission-mode`, `--allowedTools` or settings rules, never by a blanket yes. |
| Secrets | Values whose key looks secret (`*key*`, `*token*`, `*secret*`, `*password*`, `*auth*`, ...) and values that look like credentials (`sk-…`, `Bearer …`) are printed as `<redacted>` in `config` output and diagnostics. |

## Exit statuses

| Status | Meaning |
| --- | --- |
| `0` | Success. This includes a turn interrupted by an IDE host over the control channel. |
| `1` | The run failed: an API error, a tool or agent failure, or a prompt blocked by a hook. Also `config get` on a key that isn't set. |
| `2` | Usage error: an unknown flag, an invalid value, a missing prompt, or a command that needs a terminal (or input) and has none. |
| `3` | Configuration or credentials are missing or invalid: no API key, a key the endpoint refuses (HTTP 401 or 403, also mid-run), an invalid endpoint URL, a key helper that fails, an unreadable `--settings`, a session that doesn't exist, or an unknown model with a budget. Also `doctor` when it finds a problem. |
| `4` | A limit ended the run: `--max-turns` or `--max-budget-usd`. |
| `130` | Interrupted by Ctrl-C (SIGINT). The current turn's result is still written. |

## Commands

### `kodact [prompt]`: interactive session

- **Purpose:** work with the agent in a project, in conversation.
- **Example:** `kodact "add a --dry-run flag to the export command"`
- **Input:** the optional first prompt, then what you type. Slash commands
  work here and in `-p` (see "Slash commands" below); `/exit` quits.
- **Needs a terminal on stdin.** Without one, or with `--no-input`, KodAct exits
  with status `2` and suggests `-p`.
- **Side effects:**
  - files change as the agent's tools allow;
  - a session transcript and file checkpoints are written to the state
    directory;
  - each prompt you send is added to `<state>/history.jsonl` (mode 0600),
    which Up and Down browse, per project directory.

**The terminal UI** opens when stdin and stdout are both terminals. Finished
output (your prompts, answers, tool calls) goes into the terminal's own
scrollback, so scrolling and search work as in any shell. A small region at
the bottom shows the answer being written, a spinner, dialogs, queued
messages, the input box, the `/` menu and a status line (permission mode,
model, context used, cost). Design and key table: `docs/TUI.md`.

| Key | Does |
| --- | --- |
| Enter | Send. While a turn runs, the message is queued and sent when it ends |
| Shift+Enter, Alt+Enter, Ctrl+J, `\` then Enter | New line |
| Esc | Interrupt the turn; close the menu; cancel a dialog. Twice on an empty prompt: `/rewind` |
| Ctrl+C | Clear the input; interrupt the turn; twice on an empty prompt: exit |
| Ctrl+D | Exit (empty prompt) |
| Shift+Tab | Cycle the permission mode: default, accept edits, plan |
| Up / Down | Move between lines; earlier prompts from the first or last line; move in menus and dialogs |
| Tab | Complete the highlighted command or `@` path |
| Ctrl+R | Search earlier prompts; Ctrl+R again for older ones, Esc to cancel |
| Ctrl+L | Redraw |

`/model`, `/resume`, `/rewind`, `/output-style` and `/permissions` without
an argument open a picker (type to filter, Enter to choose). Each choice runs
the same command with its argument, as you could type it. Typing `@` and part
of a path lists matching project files.

**Key bindings.** `keybindings.json` in the config directory (`kodact config
paths` shows it; `$KODACT_HOME` when set) maps keys to actions:

```json
{"ctrl+s": "submit", "alt+r": "historySearch", "ctrl+g": "none"}
```

A key is modifiers (`ctrl`, `alt` or `meta`, `shift`) and a key name joined
by `+`: a character, `enter`, `tab`, `esc`, `backspace`, `delete`, `up`,
`down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `space`,
`f1`-`f12`. `"none"` unbinds a key. A bound key does what the action's
default key does, everywhere (dialogs and screens too); default keys keep
working unless they are bound to something else or to `"none"`. A bad entry
(unknown key or action, invalid JSON) is a warning at start and is skipped.
`/keybindings` (and `?`) show the keys in effect. The actions:

| Action | Default keys | Does |
| --- | --- | --- |
| `submit` | Enter | Send the prompt |
| `newline` | Shift+Enter, Alt+Enter, Ctrl+J | New line |
| `cancel` | Esc | Interrupt the turn; close a menu or dialog; twice: `/rewind` |
| `interrupt` | Ctrl+C | Clear the input; interrupt; twice on an empty prompt: exit |
| `exit` | Ctrl+D | Exit (empty prompt) |
| `cycleMode` | Shift+Tab | Cycle the permission mode |
| `historySearch` | Ctrl+R | Search earlier prompts |
| `complete` | Tab | Complete a `/` command or an `@` path |
| `lineStart`, `lineEnd` | Ctrl+A / Home, Ctrl+E / End | Start, end of line |
| `deleteWord` | Ctrl+W, Alt+Backspace | Delete the word before the cursor |
| `killToStart`, `killToEnd` | Ctrl+U, Ctrl+K | Delete to the start, end of the line |
| `wordLeft`, `wordRight` | Alt+B / Ctrl+Left, Alt+F / Ctrl+Right | Word left, right |
| `redraw` | Ctrl+L | Redraw the screen |
| `toggleSidebar` | Ctrl+B | Show or hide the sidebar (full screen, 140+ columns) |
| `showKeys` | `?` | Show the shortcuts (empty prompt) |

When you leave, KodAct prints `Resume this conversation with: kodact --resume <id>`.

**The line REPL** (`--no-tui`, or `KODACT_TUI=0`, or when stdout isn't a
terminal) reads one line at a time and prints the conversation on stdout;
warnings go to stderr. Ctrl-C interrupts the current turn without killing
the process; `/exit` or end-of-file quits. It reads input only between
turns, so a command typed during a turn runs after it. In the terminal UI,
`/status`, `/usage`, `/tasks`, `/context`, `/mcp` and `/btw` answer at once,
even mid-turn.

**`@` mentions** (every mode: TUI, REPL, `-p`, stream-json). A prompt that
names `@path` (at the start or after a space; `@"name with spaces.md"`
quoted; `~/` for the home directory, other paths from the working directory)
gets that file's contents attached, so the model needn't Read it. Your text
is kept as typed (history, `/rewind` and hooks see it so); the contents
follow in a separate system-reminder block, one `<file path="...">` each.
A directory gets a listing (at most 200 entries, `.gitignore` respected).
Images, PDFs and binary files aren't inlined: a note tells the model to use
Read. Limits: 50,000 characters per file (the middle is cut), 10 files and
200,000 characters per message; what doesn't fit gets a note. An attachment
is a read: it follows Read's permission rules without prompting, so a file
outside the working directories or matched by an ask or deny rule isn't
attached (the note says why). Attached files count as read, so the model can
Edit them at once. `@name` with no such file, and emails, are left alone. In
stream-json only a message that is a single text block is scanned. A
`/loop` prompt and each scheduled run of it are scanned too (each run reads
the files as they are then). Other slash commands and `!shell` lines aren't scanned (custom commands attach the
`@path`s in their own body).

### `kodact -p [prompt]`: print mode, for scripts and CI

- **Purpose:** run one task without supervision and print the result.
- **Examples:**
  - `kodact -p "explain src/main.rs"`
  - `git diff | kodact -p "review this"`
  - `kodact -p --output-format json --max-budget-usd 0.50 "fix the failing test"`
- **Input:** the prompt argument, piped stdin, or both. With both, stdin comes
  first, then the prompt.
- **Output, per `--output-format`:**
  - `text`: on success, the final answer on stdout; on failure, nothing on
    stdout and the error on stderr.
  - `json`: one result document (schema below), plus `exit_code`. If the run
    can't start, one error document instead.
  - `stream-json`: NDJSON. A `system/init` line, then `assistant`, `user` and
    `system` lines (plus `stream_event` lines with
    `--include-partial-messages`), and a `result` line after each turn.
- **Permissions:** tool calls that need approval are denied, and the model is
  told how to allow them (contract C1). They are listed in
  `result.permission_denials`. To allow them, use `--allowedTools`,
  `--permission-mode` or settings rules, or run shell commands in the
  sandbox (`--sandbox workspace-write`, below), where they need no approval.
- **Limits:**
  - `--max-turns N` caps model calls;
  - `--max-budget-usd X` caps spend; it fails closed when the model's pricing
    is unknown;
  - `--autonomous` (or `KODACT_AUTONOMOUS=1`) is for runs nobody watches. It
    removes the question and plan-approval tools, tells the model to finish
    without asking, reminds it once to make a real attempt if it tries to
    end a turn without using any tool, and retries model calls that fail for a
    reason that passes (rate limit, overload, server error, lost connection)
    instead of ending the run (contract C21);
  - `--max-time T` (`900`, `15m`, `1h`) tells the model its time limit up
    front, warns it when a fifth of the time (at least a minute) is left, and
    stops the run at the limit: a model call or a tool in flight is cut, the
    work done so far stays in place, and the result is an error with
    `stop_reason: "max_time"` (KodAct extension; a script gets a result instead
    of an outside kill);
  - `--fallback-model M` switches models on overload.
- **Interrupting:** Ctrl-C stops the run; the result is written and the exit
  status is `130`.
- **Worktree:** `-w/--worktree [name]` runs the session in
  `.kodact/worktrees/<name>`, on a new branch `kodact/<name>` (the name is
  generated when left out). The worktree is kept afterwards, and the main
  checkout is untouched.
- **Web:**
  - `WebFetch` answers its `prompt` from the page with the small model
    (`smallFastModel` setting), so whole pages stay out of the context.
  - `WebSearch` uses the provider's web-search server tool.
  - Both need permission. Allow sites with rules like
    `WebFetch(domain:docs.rs)`.

### `kodact -p --input-format stream-json --output-format stream-json`: IDE host protocol

- **Purpose:** let an editor or other program drive KodAct.
- **Input:** NDJSON on stdin:
  - `user` messages;
  - `control_request` messages: `initialize`, `interrupt`,
    `set_permission_mode`, `set_model`, `set_max_thinking_tokens`,
    `mcp_status`, `mcp_reconnect` (`serverName`), `mcp_toggle`
    (`serverName`, `enabled`), `rewind_files`, `stop_task`.
    `stop_task` stops one Task agent (`task_id`: its Task call's
    `tool_use_id`) or all of them (`all: true`) and answers
    `{"stopped": n}`; the turn goes on, and the model gets each agent's
    result beginning `Stopped by user`. An unknown id is answered with the
    error `No running agent <id>`. `set_permission_mode` to
    `bypassPermissions` needs `--allow-dangerously-skip-permissions` (or a
    launch in that mode) and is refused when managed settings disable it;
  - `control_response` answers to KodAct's `can_use_tool` requests.
- **Output:** NDJSON on stdout, as in print mode, plus `control_request` and
  `control_response` lines.
- **Permission prompts** go to the host when
  `--permission-prompt-tool stdio` is given, or by default. With
  `--permission-prompts none` or `--no-input` they are denied.
- **Ends** at end-of-file on stdin. `--replay-user-messages` echoes each user
  message with the uuid it was stored under; `rewind_files` takes that uuid.
- **Immediate commands:** a `user` message that is one of `/status`,
  `/usage` (`/cost`, `/stats`), `/tasks`, `/context`, `/mcp`, `/btw` (one
  text block) is answered at once, even while a turn runs, and never queues
  behind it. The answer is a `result` line with `num_turns: 0`, the current
  `session_id` and `"immediate": true`; the running turn's own result comes
  later without that field. `/btw`'s cost and `/mcp`'s effect on the system
  prompt and `system/init` are applied when the turn ends.

### `kodact doctor [--probe]`

Checks the local setup:
- settings files;
- `provider`: the Messages API or an OpenAI-compatible endpoint, and what chose it;
- `endpoint`: the URL and the variable or settings file it came from;
- `credentials`: which variable or helper supplies the key and how it is sent,
  masked to its last four characters; a key set for the other provider is
  named as the likely mistake;
- git, and the shell commands will run in;
- the sandbox, and the config and state directories.

Prints one line per check to stdout. Exits `0` when everything passes and `3`
when anything fails. Without `--probe` it makes no network calls and runs no
key helper. `--probe` adds one authenticated request that changes nothing
(the endpoint's model list), running the key helper first if there is one, to
check that the URL and key work.

### Which key goes where

| Endpoint | Chosen by | Key | Sent as |
| --- | --- | --- | --- |
| OpenAI-compatible | `KODACT_OPENAI_BASE_URL`, else `openai.baseUrl` | `KODACT_OPENAI_API_KEY`, else `openai.apiKeyHelper` | `Authorization: Bearer` |
| Gemini API | `KODACT_GEMINI_BASE_URL`, else `gemini.baseUrl`, else `KODACT_GEMINI_API_KEY` alone (Google AI Studio, when no Messages key is set) | `KODACT_GEMINI_API_KEY`, else `gemini.apiKeyHelper` | `x-goog-api-key` for Google AI Studio, `Authorization: Bearer` for Vertex AI, both for other hosts; `gemini.authHeader` (or `KODACT_GEMINI_AUTH_HEADER`) chooses: `x-goog-api-key`, `bearer`, `both` |
| Messages API | `KODACT_BASE_URL`, else `baseUrl`, else the default host | `KODACT_API_KEY` and/or `KODACT_AUTH_TOKEN`, else `apiKeyHelper` | `x-api-key` / `Authorization: Bearer` |

- A key is never sent to the other kind of endpoint. With an OpenAI-compatible
  endpoint and only `KODACT_API_KEY` set, KodAct warns at startup, and a 401
  names `KODACT_OPENAI_API_KEY`.
- The OpenAI-compatible endpoint may need no key (a local server): KodAct
  starts without one, silently for `localhost` and private addresses, with a
  warning otherwise. The Messages API always needs one.
- The Gemini endpoint's URL ends at the API version: Google AI Studio's is
  `https://generativelanguage.googleapis.com/v1beta` (the default); Vertex
  AI's is `https://<location>-aiplatform.googleapis.com/v1/projects/<project>/locations/<location>/publishers/google`,
  with `gemini.apiKeyHelper` set to `gcloud auth print-access-token`. KodAct
  adds `/models/<model>:streamGenerateContent`. Gemini 3 and later get
  effort as their thinking level, 2.5 as a thinking budget; each tool call's
  thought signature goes back on that call.
- Values wrapped in quotes (what `set X="..."` leaves in cmd.exe) are
  unquoted, with a warning. A key with spaces or control characters, or a URL
  that isn't `http(s)://`, stops startup with exit `3`.
- `baseUrl`, `openai.baseUrl`, `gemini.baseUrl`, `apiKeyHelper`,
  `openai.apiKeyHelper` and `gemini.apiKeyHelper` are
  read from your user, local, `--settings` and managed settings only. In a
  project's checked-in `.kodact/settings.json` they are ignored with a warning:
  a repository can't choose where your key goes or run a command for it.
- A key helper must print the key and nothing else on one line; any failure
  (exit status, stderr, empty or extra output) is reported and stops startup.

### `kodact config list [--origin] | get <key> | paths`

- **`list`:** the merged settings as JSON, with secrets redacted.
- **`list --origin`:** for each top-level key, its value, its source layer and
  its file.
- **`get <key>`:** takes a dotted key (`permissions.defaultMode`) or a JSON
  pointer. It prints strings raw and other values as JSON, and exits `1` when
  the key isn't set.
- **`paths`:** the config, state and cache directories, and the settings files
  that were read.

Read-only. The output goes to stdout.

### `kodact mcp ...`: MCP servers

| Command | Does |
| --- | --- |
| `kodact mcp add [-s local\|project\|user] [-t stdio\|http\|sse] [-e K=V]... [-H "K: V"]... <name> <command-or-url> [-- args...]` | Saves a server. `local` writes `.kodact/settings.local.json` (private), `project` writes `.mcp.json` (shared) and `user` writes `<config>/settings.json` |
| `kodact mcp add-json [-s scope] <name> '<json>'` | The same, from a JSON config |
| `kodact mcp remove [-s scope] <name>` | Removes it (from every scope when none is given). Exits 1 if it was not found |
| `kodact mcp list` | Starts each server and reports `connected, N tools`, `failed: <why>` or `needs approval`. Exits 1 if any failed |
| `kodact mcp get <name>` | Config (secret values hidden), status, server info and tools |
| `kodact mcp approve <name>` / `--all` | Trusts servers from this project's `.mcp.json` (local settings) |
| `kodact mcp serve` | KodAct's built-in tools as an MCP server on stdio |

**Config shapes:**
- `{"command": "...", "args": [...], "env": {...}}`
- `{"type": "http", "url": "...", "headers": {...}}`
- `{"type": "sse", "url": "..."}`

Values may use `${VAR}` and `${VAR:-default}`.

**In a session:**
- `--mcp-config <file-or-json>...` adds servers, and `--strict-mcp-config`
  uses only those.
- Server tools appear as `mcp__<server>__<tool>`, and are allowed with rules
  like `mcp__github` or `mcp__github__create_issue`.
- `system/init` lists each server's status.

### `kodact completion <bash|zsh|fish|elvish|powershell>`

Prints a completion script generated from the same definitions as `--help`.
For example: `kodact completion bash > ~/.local/share/bash-completion/completions/kodact`.

### `kodact-eval run | compare | validate | list`

Measures the harness; see `evals/README.md`. `run` writes `report.json` and
`report.md` under `evals/results/<label>/` and prints the Markdown report.
`validate` exits non-zero if any task is invalid.

## Machine output schemas

**Result** (`--output-format json`, and the final `result` line of stream-json):

```json
{"type": "result", "subtype": "success|error_during_execution|error_max_turns|error_max_budget_usd",
 "is_error": false, "result": "final answer text", "stop_reason": "end_turn|interrupted|max_turns|...",
 "num_turns": 3, "duration_ms": 8123, "duration_api_ms": 7012, "session_id": "uuid",
 "total_cost_usd": 0.0421, "usage": {"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
 "modelUsage": {"<model>": {"inputTokens": 0, "outputTokens": 0, "costUSD": 0.0}},
 "permission_denials": [{"tool_name": "Bash", "tool_use_id": "…", "tool_input": {}}],
 "errors": [], "structured_output": null, "uuid": "uuid", "exit_code": 0}
```

`exit_code` appears in `json` mode only. A turn whose last replies held only
reasoning (no text, no tool call), with the reminders to act used up, ends as
`error_during_execution` with "The model stopped without an answer: …" in
`errors` and exit code 1.

**Error before a run** (`json` and `stream-json`):

```json
{"type": "error", "error": {"message": "what failed", "hint": "the next step, or null", "exit_code": 3}}
```

The same message also goes to stderr for people.

## Configuration

**Precedence, highest first:**
1. Command-line flags.
2. Environment variables (`KODACT_MODEL`, ...).
3. Managed policy (`/etc/kodact/managed-settings.json`). An administrator's
   policy beats user files on purpose.
4. `--settings` (a file or a JSON string).
5. Project local settings: `.kodact/settings.local.json`.
6. Project settings: `.kodact/settings.json`.
7. User settings: `<config>/settings.json`.
8. Built-in defaults.

**Merging:** objects merge, arrays are concatenated, and scalars from a
higher layer replace lower ones. `--setting-sources user,project,local` limits
which file layers load.

**Invalid files** are skipped with a warning on stderr; `kodact doctor` reports
them.

**Models KodAct doesn't know** (a gateway's or a local server's): KodAct needs
the context window to compact in time, and prices to show costs.
- `modelLimits`: `{"<model id>": {"contextWindow": 131072, "maxOutputTokens": 8192}}`.
  `KODACT_CONTEXT_WINDOW` sets the window for every model without one of its own.
- An OpenAI-compatible endpoint's model list is read for the window when it
  reports one (`context_length`, `max_model_len`, `max_input_tokens`, ...;
  a whole number, also as text or a decimal). For a model KodAct doesn't know,
  every mode (the terminal UI, `-p` and the line prompt) waits up to 3 seconds
  for that list as it starts, so the first request uses the window; a slower
  list still sets it for the requests after. Order: `modelLimits`, then the
  endpoint's list, then the built-in table, then `KODACT_CONTEXT_WINDOW`, then
  a 200,000-token guess. The variable is one value for every model, so a window
  that belongs to the model wins over it: set for a small model, it never
  shrinks a large one whose endpoint reports its window. To change one model's
  window, use `modelLimits`. A LiteLLM gateway that lists no window in its
  model list is also asked at `/model/info` (`model_info.max_input_tokens`).
  And when the endpoint refuses a request as too long and states its window
  ("maximum context length is 100000 tokens"), KodAct uses that window from
  then on, the same as a listed one, and sends the request again sized to fit;
  it compacts only when the prompt itself doesn't fit. So a model's window is
  learned whatever its size, with no setting. `evals/context-window/` tests
  this end to end.
- `modelPricing`: `{"<model id>": {"input": 0.3, "output": 1.2}}`, USD per
  million tokens.
- Without these, KodAct assumes 200,000 tokens of context and 32,000 of output,
  says so once at startup, shows the cost as `cost ?` (or `$1.20+` when only
  part of it is known) and marks `/usage`'s context figure as a guess.

**Locations:**

| Kind | Default | Override |
| --- | --- | --- |
| Config (settings, `KODACT.md`, agents, commands) | `$XDG_CONFIG_HOME/kodact`, else `~/.config/kodact` (Linux), `~/Library/Application Support/kodact` (macOS), `%APPDATA%\kodact` (Windows) | `KODACT_HOME` |
| State (sessions, file history) | `$XDG_STATE_HOME/kodact`, else the platform state or local-data directory | `KODACT_HOME/state` |
| Cache | `$XDG_CACHE_HOME/kodact`, else the platform cache directory | `KODACT_HOME/cache` |
| Project | `.kodact/` in the project, plus `KODACT.md` files from the root down to the cwd (a directory's `AGENTS.md` when it has no `KODACT.md`). Agents (and later commands, skills and output styles) are also read from the vendor-neutral `.agents/<kind>/`, below `.kodact/<kind>/` in precedence | — |

**Environment variables:**

| Variable | Purpose |
| --- | --- |
| `KODACT_API_KEY` | Messages API key |
| `KODACT_AUTH_TOKEN` | Bearer token, for gateways |
| `KODACT_BASE_URL` | API endpoint |
| `KODACT_CUSTOM_HEADERS` | Extra headers, one `Name: value` per line |
| `KODACT_OPENAI_BASE_URL`, `KODACT_OPENAI_API_KEY` | An OpenAI-compatible endpoint instead |
| `KODACT_GEMINI_API_KEY`, `KODACT_GEMINI_BASE_URL`, `KODACT_GEMINI_AUTH_HEADER` | Google's Gemini API instead (Google AI Studio by default, Vertex AI or a gateway by URL) |
| `KODACT_MODEL` | Default model |
| `KODACT_TUI` | `inline` keeps the interactive UI in an inline viewport at the bottom of the terminal, writing output into its scrollback as it happens; by default the UI takes the whole window and prints the conversation into the scrollback when it closes |
| `KODACT_MAX_OUTPUT_TOKENS` | Per-request output cap |
| `KODACT_CONTEXT_WINDOW` | Context window, in tokens, for a model with none of its own: not in `modelLimits`, the endpoint's list or the built-in table |
| `KODACT_MAX_THINKING_TOKENS` | Thinking budget |
| `KODACT_MAX_RETRIES` | API retries (default 3) |
| `KODACT_MAX_CONCURRENT_REQUESTS` | Model requests in flight at once for the whole session (default 4; also `maxConcurrentRequests`) |
| `KODACT_MAX_PARALLEL_AGENTS` | Task sub-agents running at once (default 4; also `maxParallelAgents`) |
| `KODACT_PROMPTS_DIR` | A local prompt set |
| `KODACT_HOME` | One root for config, state and cache |
| `KODACT_SANDBOX` | Same as `--sandbox` |
| `KODACT_SHELL` | The shell commands run in: a full path, or a name on PATH (bash, sh, zsh, pwsh, powershell). Also read from the settings `env` block. See "Shells" below |
| `KODACT_VERIFY` | `0` turns the verification loop off |
| `KODACT_REMINDERS` | `1` turns KodAct's own reminders to the model back on (off by default; see `reminders`) |
| `KODACT_DISABLE_CRON` | `1` turns scheduled prompts off (`/loop`, `CronCreate`, ...) |
| `MCP_TIMEOUT`, `MCP_TOOL_TIMEOUT` | MCP connect and call timeouts, in milliseconds (30 s, 10 min) |
| `KODACT_NO_INPUT` | Same as `--no-input` |
| `KODACT_TUI` | `0`, `false`, `off` or `no`: interactive mode uses the line REPL (same as `--no-tui`) |
| `KODACT_LOG` | Log filter, with `--debug` (`/debug` turns logging on partway through a session) |
| `NO_COLOR`, `FORCE_COLOR`, `CLICOLOR_FORCE` | Color |

## Slash commands

A prompt that starts with `/name` (in any mode) is a command. `/help` lists
them all. The built-ins so far:

| Command | Does |
| --- | --- |
| `/help` | Lists every command, with aliases, then custom commands, skills and MCP prompts |
| `/status` | Version, session, directories, model, permission mode, sandbox, API key source, settings files, MCP and hooks |
| `/usage` (`/cost`, `/stats`) | Cost, time, model calls, tool calls, tokens per model, and context now |
| `/compact [what to keep]` | Summarizes the conversation now |
| `/clear [name]` (`/reset`, `/new`) | Starts a new conversation (a new session id); the old one stays resumable, under `name` if given |
| `/doctor` (`/checkup`) | `kodact doctor` plus this session's MCP, model and warnings |
| `/skills`, `/agents`, `/memory`, `/hooks`, `/plugin` | What is loaded, and from where |
| `/hooks add <Event> <matcher> <command> [--scope local\|project\|user] [--timeout <s>]` | Adds a command hook to that scope's settings file (default `local`, `.kodact/settings.local.json`) and reloads the session so it applies at once. `''` or `*` as the matcher matches everything; the words after the matcher are the command |
| `/agents create <name> --description <text> [--prompt <text>] [--tools a,b] [--model <model\|inherit>] [--scope project\|user]` | Writes `<name>.md` (project: `.kodact/agents/`, user: the config directory's `agents/`) and reloads, so the Task tool offers it at once. No tools picked means all tools; no instructions means instructions from the description. Names are lowercase letters, digits and dashes; an existing file is never overwritten |
| `/hooks remove <Event> <n>` | Removes hook `n` of that event, as `/hooks` numbers it (each line shows its source: user, project, local, flag, managed or plugin). Only user, project and local hooks can be removed |
| `/mcp [reconnect\|enable\|disable <server\|all>]` | Each MCP server's status. `reconnect` restarts a server from the config it started with; `disable` stops it, hides its tools and prompts, and saves that in `.kodact/settings.local.json` (`disabledMcpjsonServers`); `enable` reverses it. Tools a server didn't offer when the session started join after `/reload-plugins`. `--mcp-config` and plugin servers change for the session only |
| `/tasks [stop <id>]` (`/bashes`) | Background shells, scheduled tasks and subtasks; `stop` ends one |
| `/subtask <task>` | Hands the task to a background agent that starts from this conversation; you keep going. Its report reaches the model with your next prompt (and is shown when it finishes). In `-p`, KodAct waits for it, then runs one more turn with the report |
| `/model [model]` | Shows the current model and the models your endpoint lists (`GET {baseUrl}/models` on an OpenAI-compatible endpoint; the default backend lists none), or switches to the given model id |
| `/effort [low\|medium\|high\|xhigh\|max\|auto]` | Shows or sets reasoning effort; `max` lasts for the session only |
| `/fast [on\|off]` | Fast mode, on models that offer it |
| `/config [key=value ...] [--scope user\|project\|local]` (`/settings`) | Shows the settings KodAct can change, or changes them; `key=` restores the default; `/config --help` lists the keys |
| `/output-style [style]` | Lists the output styles, or switches |
| `/autocompact [on\|off\|auto\|<tokens>]` | When the conversation is summarized automatically |
| `/sandbox [on\|off\|read-only\|workspace-write]` | The OS sandbox for shell commands |
| `/permissions` (`/allowed-tools`) | Lists rules (with the layer each comes from) and working directories; `add <allow\|ask\|deny> <rule> [--scope local\|project\|user\|session]`, `remove <rule>` |
| `/add-dir <path> [--save]` | Adds a working directory; `--save` keeps it in local settings |
| `/rename [name]` | Names the session; without a name, a small model suggests one |
| `/export [file]` | The conversation as plain text, to a file or stdout |
| `/diff` | Uncommitted changes (`git diff HEAD` and untracked files), or outside git the files KodAct changed; then the files each prompt changed |
| `/context [all]` | Estimated tokens for the system prompt, tools, skills, memory and messages against the window, with suggestions |
| `/rewind [<n> <action> [instructions]]` (`/checkpoint`, `/undo`) | Lists your prompts; then goes back to before prompt `n`. Actions: `both` (code and conversation, the default), `conversation`, `code`, `summarize-from`, `summarize-to` |
| `/branch [name]` | Continues in a copy of the conversation under a new id; the original stays as it was |
| `/resume [n\|id\|name]` (`/continue`) | Interactive only: lists this directory's conversations, or switches to one |
| `/cd <directory>` | Moves the conversation to another directory (a new session there) |
| `/reload-skills`, `/reload-plugins` | Re-reads skills, commands, agents, output styles, plugins and settings without leaving the conversation |
| `/loop [interval] [prompt]` | Runs a prompt now and on a schedule (`5m`, `2h`, `every 20 minutes`), or self-paced when no interval is given (the model picks when to check again, or stops). No prompt: `.kodact/loop.md`, else a maintenance pass. `/tasks` lists scheduled tasks; `/tasks stop <id>` ends one |
| `/goal [condition\|clear]` | Sets a goal and keeps working until a check finds it met (see below) |
| `/plan [description]` | Plan mode; with a description, starts planning it |
| `/btw [question]` | A side question, answered from the conversation without tools; it doesn't enter the conversation |
| `/recap` | One line: what was asked, what's done, what's open |
| `!command` | In the interactive session, runs `command` in the shell as you (no prompt, no sandbox) and gives the model the command and its output; the model answers unless `respondToBashCommands` is `false`. In `-p`, `!...` is an ordinary prompt |
| `/advisor [model\|off]` | Lets KodAct consult a second model (the `Advisor` tool) before risky changes, when stuck, and before calling work done. Its cost counts toward the session |
| `/import [codex\|gemini\|cursor] [--yes]` | Shows what it would bring over from other coding agents (their MCP servers, GEMINI.md, Cursor rules); `--yes` applies it. Servers from your own configs go to your user settings; servers shipped in the repository go to `.mcp.json` and still need `kodact mcp approve` |
| `/feedback [description]` (`/bug`, `/share`) | Saves a bug-report bundle in `<state>/feedback/` (report, doctor checks, this session's transcript and settings, with secrets masked). Nothing is uploaded |
| `/debug [problem]` | Turns on a debug log for the session (`<state>/debug/<session>.txt`); with a description, KodAct reads the log and diagnoses it |
| `/release-notes` | The changelog |
| `/color [name\|default]` | Terminal UI only: the accent colour (prompt and answer markers, selections, dialog borders) for this session: red, orange, yellow, green, cyan, blue, purple, pink; `default` is KodAct's own. Not saved |
| `/focus [on\|off]` | Terminal UI only: keeps tool calls and their results out of the scrollback until turned off (the spinner still names the running tool) |
| `/diff`, `/context`, `/hooks`, `/agents` with no arguments | Terminal UI: a screen instead of the text answer (TUI.md, "Screens"): the diff viewer, the context grid, the hooks editor, the agents wizard |
| `/theme [style\|dark\|light\|none]` | Terminal UI only. A style (`frame`, `glow`, `card`, `ledger`, `status`, `bare`; ForgeCLI's `forgeframe`, `molten` and `anvil` still work) sets the front page, chat lines, approvals and prompt, saved as `tuiStyle`; `auto`, `dark`, `light` or `none` set the colours, saved as `theme`; both in user settings. Without an argument, a picker of both. `NO_COLOR` still wins |
| `/copy [n]` | Terminal UI only: the latest answer (or the nth latest) to the clipboard through the terminal (OSC 52; inside tmux, `set -g set-clipboard on`) |
| `/keybindings` | Terminal UI only: the keyboard shortcuts (also `?` on an empty prompt) |
| `/statusline [command\|off]` | Terminal UI only: shows the first output line of `command` on the right of the status line. It runs after each turn (at most 3 s) with the session as JSON on stdin: `session_id`, `cwd`, `model.id`, `model.display_name`, `workspace.current_dir`, `cost.total_cost_usd`, `context.used_percentage`, `permission_mode`, `version`. Saved as `statusLine` in user settings; off while `disableAllHooks` is set |
| `/terminal-setup` | Terminal UI only: whether Shift+Enter works here, and how to get it (or use Alt+Enter, Ctrl+J or `\` Enter) |
| `/exit` (`/quit`) | Ends the session |
| `/<custom> [args]` | A custom command from `commands/*.md` |
| `/<skill> [args]` | Loads a skill; `/a /b text` chains up to six |
| `/mcp__<server>__<prompt> [args]` | An MCP server's prompt |

`docs/PARITY.md` ("Slash commands") tracks the rest of the reference's
commands.

**Scheduled prompts:** `/loop` and the `CronCreate` tool schedule prompts in
the session. They run only between turns, recurring ones expire after
seven days, and `-p` keeps running until they are done. It stops on Ctrl-C,
at `--max-turns` (counted across the whole run) or when the budget is spent;
a limit that stops it while tasks are still scheduled exits with status 4.
Contract C19 has the details.

**Goals:** `/goal <condition>` sends the condition as the prompt. After each
turn the model checks the conversation against it, trusting tool output rather
than the agent's own summary (a pass must cite the evidence), and KodAct keeps going
until the check says it is met or can't be met. It pauses after three turns
without a tool call, an interrupt or an error, and resumes with your next
prompt. In `-p` the whole loop runs in one invocation: one `result` per turn
plus `system/goal` events in stream-json, and exit status `1` if the goal
ends unmet. `/goal` shows its status; `/goal clear` ends it. Contract C18
has the details.

**Saving defaults:** `/model`, `/effort`, `/fast`, `/output-style`,
`/autocompact` and `/sandbox` apply at once. In the REPL (and later the
TUI) they also save the choice as the default: `model`, `effortLevel`,
`fastMode` and `autoCompact*` in user settings, `outputStyle` and
`sandbox.mode` in local project settings. In `-p` and stream-json they last
for that session only, so a script never changes your defaults. `/config`
always saves, and says when a higher-precedence layer overrides the value it
wrote.

**Local commands** answer without calling the model: the result has
`num_turns: 0`, and in text mode the answer goes to stdout. A failed or
unknown command (`Unknown command: /name`) goes to stderr with exit status
`1`. Account and cloud commands such as `/login` don't exist in KodAct.

A word that looks like a path (`/usr/bin/env ...`, or any name containing
`/`) is sent as an ordinary prompt.

**Custom commands** are Markdown files in `<config>/commands/`,
`.agents/commands/` or `.kodact/commands/` (later wins). A file in a
subdirectory is named `dir:name`, and a plugin's command `plugin:name`.

The optional frontmatter takes `description`, `argument-hint` and
`allowed-tools`. In the body:
- `$ARGUMENTS` and `$1`..`$9` are replaced by the arguments;
- ``!`cmd` `` runs `cmd` and inserts its output, only when `allowed-tools`
  permits it (e.g. `Bash(git status:*)`);
- `@path` attaches a file (same rules as `@` mentions in prompts, without the
  permission check: the command's author chose the files).

**Bundled skills** ship with KodAct:
- `/init` writes KODACT.md.
- `/code-review [low|medium|high|max] [--fix] [pr|branch|path]` (also
  `/review`) and `/security-review` review changes.
- `/simplify`, `/verify` and `/run` check and tidy work.
- `/run-skill-generator` writes a project `/run` skill.
- `/batch <change>` spreads one change over many files with sub-agents.
- `/fewer-permission-prompts` proposes safe allow rules.
- `/update-config` edits settings files.

A skill of yours with the same name replaces one, and so does
`KODACT_PROMPTS_DIR/skills/<name>.md`.

**Skills** are folders `skills/<name>/SKILL.md`, with frontmatter `name` and
`description`. The model sees only names and descriptions, and loads a body
with the `Skill` tool when the task matches. You can load one with
`/<name>`.

**Output styles:** set `outputStyle` to `default`, `explanatory`, `learning`,
or a custom style from `output-styles/*.md`.

**Plugins:** `--plugin-dir <dir>...` (or the `pluginDirs` setting) loads a
plugin directory: `plugin.json` (or `.kodact-plugin/plugin.json`) plus any of
`commands/`, `agents/`, `skills/`, `output-styles/`, `hooks/hooks.json` and
`.mcp.json`.

## Verification loop

When a turn has changed files and no check has run since the last change, the
model is reminded once to run the project's checks before it finishes, and to
say in its answer what it verified (contract C12 in `docs/ARCHITECTURE.md`).
Each reminder costs one more model call; it's recorded as `system/verification`
in stream-json and in the session.

**Check commands** come from `verification.commands`, or are detected from the
project's manifests: `cargo build` and `cargo test`; the package.json
`typecheck`, `lint`, `build` and `test` scripts, run with the lockfile's
package manager; `go build ./...` and `go test ./...`; pytest or unittest;
`make check` and `make test`; `./build.sh`, `./test.sh` and others. They're
listed in the system prompt's environment section.

| Setting | Default | Meaning |
| --- | --- | --- |
| `reminders` | `false` | `true` turns on KodAct's own reminders to the model: the verification reminder, the loop guard, the act, attempt, next-step and todo reminders, the budget and time wrap-up, and the warning about background commands. Off, the model works on its own between tool results; only what you and the CLI did is told to it (plan mode, a mode switch, attached files, a subtask's result). `KODACT_REMINDERS=1` does the same. |
| `verification.enabled` | `true` | `false` turns the loop off (as does `KODACT_VERIFY=0`), when `reminders` is on |
| `verification.commands` | detected | The project's check commands |
| `verification.maxReminders` | `1` | Reminders per user turn |

## Security

**Dangerous commands are never approved automatically.**
- A Bash command matching a high- or critical-risk pattern always asks (and
  is denied in `-p` runs and in `dontAsk` mode), whatever allow rules,
  `acceptEdits`, `auto` or the sandbox would say. The patterns cover:
  - recursive deletion of `/` or home;
  - disk overwrite;
  - a downloaded script piped to a shell;
  - reverse shells and fork bombs;
  - credential files sent over the network;
  - decoded payloads piped to a shell;
  - startup-file, cron and service persistence;
  - sudoers and setuid changes.
- The prompt says why the command was flagged, and offers no "always allow".
- Deny rules still apply first.
- `--dangerously-skip-permissions` (`bypassPermissions`) still skips the
  check, as its name says.

**Injected instructions in tool output are marked as data.**
- This applies when a file, web page or command output contains text that
  reads like instructions to an AI agent: "ignore previous instructions",
  chat-template tokens, fake system tags, or invisible Unicode tag
  characters.
- KodAct then appends a note to that tool result, telling the model the text
  is data and not to follow it. Nothing is removed. This is OWASP LLM01:
  prompt injection.

## Shells

Every command KodAct runs (the Bash tool, `!command`, hooks, `apiKeyHelper`,
the status line, `!` lines in custom commands) runs in one shell, chosen at
startup:
1. `KODACT_SHELL`, from the environment, else from a settings `env` block. A
   value that names no program, or a program KodAct can't drive (cmd, fish),
   is an error, never silently skipped.
2. **Windows:** Git Bash: next to `git.exe` on PATH, then the standard install
   directories (`%ProgramFiles%\Git`, `%LOCALAPPDATA%\Programs\Git`, scoop),
   then a `bash.exe` on PATH that isn't the WSL launcher. Without Git Bash:
   PowerShell 7 (`pwsh`), then Windows PowerShell 5.1. cmd.exe is never used.
   **Unix:** bash, then sh.

The tool keeps the name `Bash` (permission rules and hooks match on it); its
description and the model's environment prompt say which shell it is, and on
Windows how to write paths for it.

**With no shell,** KodAct starts anyway: it warns once on stderr, `kodact doctor`
fails its `shell` row with the places it looked, and the Bash tool answers
every call with that message and tells the model not to retry, instead of
failing with an OS error.

On Windows, a command is stopped with everything it started (a job object), so
the timeout, Esc and KillShell end it; paths are shown without the `\\?\`
prefix; and Git Bash paths (`/c/Users/me`) are understood by the file tools
and permission rules, which compare paths without regard to case.

## Sandbox

`--sandbox <mode>`, `KODACT_SANDBOX` or the `sandbox.mode` setting (in that
order) run each Bash command inside an OS sandbox. The sandbox is separate from
permissions: a sandboxed command is allowed without a prompt, because the
operating system, not a rule, limits what it can touch.

| Mode | Writable | Network |
| --- | --- | --- |
| `off` (default) | everything the user can write | yes |
| `read-only` | the temp directories only | `sandbox.network` (default off) |
| `workspace-write` | the working directories (`--add-dir` included), the temp directories, and `sandbox.writableRoots` | `sandbox.network` (default off) |

**What stays protected, in every mode:**
- `.kodact/` inside each writable root (settings, hooks and agents can't be
  rewritten from a command);
- `.git/hooks` and `.git/config`.

**The rules still apply:**
- deny rules still deny;
- paths outside the working directories still ask.

**Backends:**
- Linux: bubblewrap (`bwrap`). Install it with your package manager.
- macOS: `sandbox-exec` with a generated Seatbelt profile. This backend is
  untested.
- Windows: none. Run KodAct inside WSL 2 to confine commands.

`kodact doctor` reports which backend was found.

**When the backend is missing,** KodAct warns once on stderr and commands go
through normal approval. It never runs them unconfined silently.

**When a command fails because of the sandbox,** the tool result says so. The
model can retry it with `dangerouslyDisableSandbox: true`; that call runs
unconfined and goes through normal approval (denied in `-p` unless a rule
allows it).

Settings example:

```json
{ "sandbox": { "mode": "workspace-write", "network": false, "writableRoots": ["/home/me/.cache/pip"] } }
```

## Pipes

- **A closed stdout** ends KodAct quietly with status 0. That happens with
  `kodact ... | head`, or when a host exits.
- **SIGPIPE stays ignored**, so a hook or MCP server that exits before
  reading its input can't kill the process.

## Rate limits

Every model request in a session (the main agent, Task sub-agents,
`/subtask`, compaction, goal checks, `/btw`) shares one limit on requests in
flight: `maxConcurrentRequests` or `KODACT_MAX_CONCURRENT_REQUESTS`, default 4.
A request over the limit waits for a slot instead of failing.

When the endpoint answers 429 anyway, KodAct halves the limit (never below 1),
says so once on stderr or in the UI, waits (`Retry-After` when the endpoint
sends one, otherwise 2, 4, 8... seconds, at most 60) and sends the request
again, up to 6 times on top of the provider's own retries. After 8 requests in
a row succeed, the limit grows back by one.

`maxParallelAgents` (or `KODACT_MAX_PARALLEL_AGENTS`, default 4) caps how many
Task sub-agents run at once; the others wait their turn. A sub-agent that
fails returns a result starting with `FAILED:` that tells the model its task
is not done.

## Network behaviour

**Timeouts:**
- connecting: 30 s;
- between reads of a response: 300 s (`KODACT_MAX_RETRIES` controls retries);
- no overall deadline, because long generations stream for minutes.

**Retries:** only failures before any streamed output are retried, at most
`KODACT_MAX_RETRIES` times, with exponential backoff. They cover HTTP 408, 409,
429 and 5xx (honouring `retry-after`) and connection failures.

**Errors** state what failed, the cause, and the next step. For example:
`could not connect to http://…/v1/messages: Connection refused. Check KODACT_BASE_URL, your network connection and proxy settings.`

**Proxies and certificates:** every HTTPS connection (the model API, WebFetch,
HTTP and SSE MCP servers) goes through `HTTPS_PROXY` / `NO_PROXY` when set, and
trusts both the built-in public roots and the system's certificate store,
including a file named by `SSL_CERT_FILE` or a directory named by
`SSL_CERT_DIR`. So behind a TLS-inspecting company proxy, KodAct trusts the
proxy's CA wherever curl and Python on the same machine do.
