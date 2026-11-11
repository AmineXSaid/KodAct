# Changelog

## 0.1.0 (unreleased)

### Failed commands explained, and built programs run
- **`command not found` (exit 127) names what is installed instead**, by job:
  `xxd` missing → `od`; `python3` missing → `perl`; `jq` missing → none of
  `jq`, `python3`, `node` either. The model uses one at once instead of
  probing for tools one call at a time (minimal containers, fresh boxes).
- **Exit 137 says the command was killed** (SIGKILL), which, unless you
  stopped it, is almost always the system ending a process out of memory, so
  the same command isn't run again as is.
- **A program built and never run is run once before finishing**: when the
  last passing check compiled a program (`gcc -o image image.c`, a plain
  `gcc x.c` making `a.out`, `rustc`, `go build -o`) and nothing ran it since,
  the verification check asks once to run it and look at its output.
  Building and running in one command, or running the tests, needs nothing.

### The "to build" list in GOALS.md
- **Notes that outlast the context** (the Notes tool): given back after
  every compaction and kept in `<session>.notes.md` next to the transcript.
- **Checkpoints the model goes back to itself** (the Checkpoint tool):
  `save` before an approach that may fail, `restore` to put the files back
  as they were. In a git repository a checkpoint is the whole working tree,
  shell changes included, kept under `refs/kodact/checkpoints/` without
  touching the index or a branch, and it survives a resume.
- **Formatting after edits**, opt-in (`verification.format: true`): the
  edited file is run through its formatter and the result says what moved.
  Only a file that was already in the formatter's style is touched.
- **The language server tells only the errors an edit added**
  (`verification.languageServer`): the file before the edit is the baseline.
  A file a shell command rewrote is checked too, when the session had
  already had it checked.
- **Errors that say the next step:** a missing path names the nearest
  directory and the close names in it; an empty Glob or Grep says what was
  searched and the usual cause. LS and NotebookEdit return structured
  results, as every other tool does.
- `scripts/eval-descriptions.sh` measures a set of tool descriptions against
  the built-in ones.

### Three gaps closed from a source comparison with other harnesses
- **A session killed mid-turn says so when resumed.** The transcript records
  where each turn starts and ends. Resuming one whose last turn never ended
  tells the model which tool calls returned nothing and to check the real
  state before continuing.
- **Sub-agents can work in their own worktree.** `isolation: "worktree"` on
  a Task call gives the agent a copy of the last commit on a new branch, so
  agents changing files in parallel can't overwrite each other. The result
  names the branch when something changed.
- **Edit by line range** (`edit.lineRanges`, off by default). Edit can name
  the lines to replace, with the first line's text as an anchor, instead of
  retyping them.

### Checks on the agent's work, second round
From `docs/VERIFICATION_COMPARISON_2.md`.
- **Language-server diagnostics after edits**, opt-in
  (`verification.languageServer: true`): the errors `rust-analyzer`,
  `typescript-language-server`, `pyright-langserver` or `pylsp`, `gopls` or
  `clangd` report for the edited file's new text are added to the edit's
  result, other files' included. Off by default; a missing or slow server
  adds nothing.
- **A fallback model takes over on a limit that doesn't clear**: after three
  rate limits waited out in a row, on a `retry-after` over a minute, and on a
  spent quota, as it already did on overload.
- **The loop guard reminds again** at 5 and at 8 when the same call goes on.
- **A reply that repeats itself is stopped while it streams**, and the model
  is told, instead of running to the output cap.
- **A tool call written as text is run** (`<tool_call>…</tool_call>`, or a
  reply that is only the call's JSON), as small and local models write them.
- **`/goal` works with these checks:** its check is told what KodAct recorded
  of the turn's changes (unchecked files, the last check's outcome); a turn
  whose calls were all refused counts as a turn without a tool; a `-p` run
  with a goal gets no end-of-run todo reminder.

### Checks on the agent's work
From `docs/VERIFICATION_COMPARISON.md`, a comparison with four other CLIs.
- **A spent quota is not retried.** A 402, or a 429 or 403 that says the
  credit or spending limit is used up, is shown at once with what to do,
  instead of ten tries a minute apart. A plain 429 is still waited out.
- **A long `retry-after` is not shortened.** When the endpoint asks for more
  than a minute, KodAct says how long ("asks to wait 48m") and ends the turn.
  Unattended runs take the wait in full when it fits `--max-time`, or up to
  ten minutes with no limit.
- **The result says whether the changes were checked:** `verification.status`
  in the `json` and stream-json result is `passed`, `failed`, `unchecked`,
  `read_back` or `no_changes`, with the check's command or the unchecked
  files. A script can gate on it.
- **A file an edit breaks is said at once.** When a file no longer parses
  after an Edit, MultiEdit or Write, the edit's result says so, using the
  language's own parser when installed (Python, JavaScript, shell, Ruby, PHP,
  JSON). `verification.syntaxCheck: false` turns it off.
- **The loop guard stops a repeat it warned about.** After its reminder, the
  same failing call to a file tool is answered "Not run" with the last error
  until something changes. Shell, web and MCP calls are never refused.
- **`-p` runs don't end quietly with todos open.** A run that takes no further
  input is sent back once to finish its open TodoWrite items or say what is
  left.

### A new mark: from code to impact
- KodAct's mark is a step and a chevron, "from code to impact", and the
  front pages draw it, in KodAct's lime. The taglines say "From code to
  impact", and the wordmark's A is pointed. The logo files in
  `design/brand` are redrawn (the name in Outfit), and kodact.exe carries
  the new icon.

### Fixed
- `/clear` (and `/reset`, `/new`) left the old conversation on screen in the
  terminal UI: only the answer said it was cleared. Now the screen starts
  over, on the front page, with the conversation it left as the one to
  `/resume`. `/resume` and `/rewind` had the same fault and now show the
  conversation they moved to, as does starting with `--resume` or
  `--continue`, which showed an empty screen.

### On crates.io
- `cargo install kodact-cli` installs the `kodact` command (and
  `cargo install kodact-eval` the eval harness); the other `kodact-*` crates
  are published as its libraries. `/release-notes` shows this file there too.

### ForgeCLI is now KodAct
- The command is `kodact`, the crates are `kodact-*`, the settings are
  `KODACT_*` variables, `.kodact/` and `KODACT.md`, and the user folders are
  `~/.config/kodact` (state and cache likewise). Entries below say "Forge":
  they are from before the rename.
- Nothing set up before is lost: every `FORGE_*` variable still works (as
  its `KODACT_*` name, when that is unset); a project's `.forge/` and its
  `FORGE.md`, and the user's `~/.config/forge`, state and cache folders, are
  used until KodAct's own exist; `.forge/<kind>` resources are still read;
  `${FORGE_*}` placeholders in skills and plugins still fill in.

### Skills written for the reference CLI
- They load unchanged: Forge also reads `~/.claude/skills` (or
  `$CLAUDE_CONFIG_DIR/skills`) and the project's `.claude/skills` from the
  repository root down. The same goes for commands, agents and output
  styles. A plugin's `.claude-plugin/plugin.json` is its manifest too.
- A skill's text is filled in before the model reads it: `$ARGUMENTS`,
  `$ARGUMENTS[N]` and `$N` (from 0, with quoting: `/migrate "Button group"`),
  `$name` from `arguments:`, and the skill's, project's and plugin's folders
  (`${FORGE_SKILL_DIR}`, or `${CLAUDE_SKILL_DIR}` as such skills write it).
  Before, those reached the model as literal text.
- Frontmatter reads as there: `allowed-tools: Bash(git add *) Bash(git commit *)`
  is two rules, not five words; a skill without a description uses its first
  line; `when_to_use` joins the description.
- A skill's frontmatter now acts as there. Its ``!`command` `` lines and
  ` ```! ` blocks run first and their output takes their place (allowed by
  its `allowed-tools` or your rules; a deny rule or a failure stops it).
  For the turn it runs in, `allowed-tools` needs no prompt, `disallowed-tools`
  is hidden, and `model` and `effort` apply; its `hooks` stay for the
  session. `context: fork` runs it in a sub-agent (`agent:`) and brings back
  the report.
- Which skill runs: yours replaces a built-in command or a command file of
  the same name; names match regardless of case. Skills in a subfolder's
  `.claude/skills` appear once a file there is used, `--add-dir` folders'
  skills load, `paths:` holds a skill back until a matching file is used,
  and `skillOverrides` and `disableSkillShellExecution` are read. Loaded
  skills come back after compaction.
- Plugins: extra component paths in the manifest, and a `SKILL.md` at the
  plugin's root.

### Windows
- Commands run on Windows: Forge finds Git Bash (next to `git.exe`, the
  standard install directories, scoop), else PowerShell; `FORGE_SHELL` picks
  one. Hooks, `apiKeyHelper`, the status line and custom-command `!` lines use
  the same shell. The Bash tool's description and the environment prompt name
  the shell and how to write paths for it.
- No shell is a clear, non-retryable answer naming where Forge looked and the
  fix, a startup warning and a failing `forge doctor` row, instead of an OS
  error the model kept retrying.
- Timeouts, Esc and KillShell stop a command with everything it started (a job
  object). Paths are shown without the `\\?\` prefix. File tools and
  permission rules accept Git Bash paths and compare paths without regard to
  case, as the filesystem does. `forge doctor` finds `git.exe`.

### Gemini and gateways (OmniRoute)
- Gemini 3 tool loops work through OpenAI-compatible endpoints: each tool
  call's thought signature (`extra_content`, or a gateway's
  `thought_signature`) is kept and sent back on that call. Before, the
  second step of any tool loop failed with "Function call is missing a
  thought_signature".
- Through OmniRoute, tool-call ids go back exactly as issued and a
  message-level signature goes back on its message; Gemini's reasoning goes
  back as `reasoning_content`.
- Parallel tool calls an endpoint sends at the same `index` are kept apart.
- Gemini models get tool names, schemas and effort they accept: long MCP
  tool names are aliased, schemas trimmed to Gemini's subset, effort sent as
  `reasoning_effort`, `<thought>` tags read as thinking.
- A native Gemini wire: `FORGE_GEMINI_API_KEY=... forge -m
  gemini-3-flash-preview` talks to Google AI Studio directly;
  `FORGE_GEMINI_BASE_URL` points it at Vertex AI or a gateway. Thought
  signatures go back on the parts they came with, effort becomes Gemini's
  thinking level (or 2.5's budget), PDFs go as files, a malformed function
  call is asked again once, and Google's retry delay is honoured. `forge
  doctor` shows the Gemini endpoint and key.
- Switching to a Gemini 3 model mid-session (`/model`), or resuming an old
  session on one, no longer fails on the first tool call: calls Gemini
  never signed get Google's placeholder signature, and one model's
  signatures are never sent to another.

### OpenAI-compatible endpoints
- OpenAI's reasoning models (the o-series, GPT-5 and Codex) are sent
  `max_completion_tokens` and no `temperature`, also behind a gateway's
  provider prefix (`cx/gpt-5-codex`, `openai/o3` through OmniRoute,
  OpenRouter or LiteLLM). They refused `max_tokens` with a 400.
- A 400 that refuses a parameter Forge sets (`max_tokens` for
  `max_completion_tokens` or back, `temperature`, `stream_options`) is
  answered by sending the request once more as asked, and remembered for
  that model's later turns. A value refused ("max_tokens is too large") is
  left alone.
- When both length names are refused (a gateway that turns
  `max_completion_tokens` back into `max_tokens`, as OmniRoute does for some
  providers), the request goes without a length limit.
- Endpoints that stray from OpenAI's stream are read anyway: a turn with tool
  calls ends in tool use even when the stream says `stop` or nothing; a call
  without an id gets its own; arguments sent as a JSON object are kept (they
  were read as empty); and a whole JSON completion in answer to a streaming
  request reads as the stream would have ("stream ended without a message").

### Auth
- Each endpoint gets only its own key: `FORGE_API_KEY` and `FORGE_AUTH_TOKEN`
  never reach an OpenAI-compatible endpoint, and `FORGE_OPENAI_API_KEY` never
  reaches the Messages API. A key set for the wrong one is named at startup.
- Errors name the URL that answered and the variable the active endpoint
  reads: a 401 from an OpenAI-compatible endpoint points at
  `FORGE_OPENAI_API_KEY`, a 404 or connection failure at the URL variable in
  use.
- A refused key (401/403) exits with `3`, also mid-run and in JSON modes.
- `forge doctor` reports the provider, the URL and where it came from, and
  the key's source, masked; `forge doctor --probe` checks them with one
  request. `/status` shows the same.
- Quoted values are unquoted; an invalid URL or key, or a failing key helper,
  stops startup with a clear message. `openai.apiKeyHelper` is new.
- Project settings can no longer choose the endpoint or run a key helper.

### Models and context
- `modelLimits` (and `FORGE_CONTEXT_WINDOW`) set the context window and output
  cap of models Forge doesn't know; an OpenAI-compatible endpoint's model list
  is read for them too. Without either, Forge says once that it is guessing.
- Compaction measures the request about to be sent, not the last response: a
  batch of large tool results no longer pushes a turn, or a sub-agent, past
  the window. Servers that report no usage get an estimate instead of 0.
- Context-overflow errors are recognised in the wording OpenAI-compatible
  servers use (`maximum context length`, `context_length_exceeded`, ...), so
  the conversation is compacted and retried instead of failing.
- Costs of models without a known price show as unknown (`cost ?`, `$1.20+`,
  `price unknown` in `/usage`) instead of `$0.00`.

### Rate limits
- One limit on model requests in flight for the whole session
  (`maxConcurrentRequests`, default 4): requests wait for a slot instead of
  failing. A 429 halves the limit, says so, waits (honouring `Retry-After`)
  and retries; the limit grows back after a run of successes.
- The OpenAI-compatible provider honours `Retry-After` and
  `FORGE_MAX_RETRIES`, and stops waiting when interrupted.
- Unattended runs (`--autonomous`) don't end on an announced step: a reply
  without a tool call whose last line says what comes next ("Let me check the
  logs.") is sent back to do it, and a run that would end with TodoWrite items
  open is shown the list. Each at most twice a turn. The unattended prompt also
  says to install missing tools, to give long commands a longer timeout or the
  background (not `| tail`), and the tools prompt to fix a failed call's cause
  before retrying.
- Unattended runs (`--autonomous`) retry a model call that failed for a
  reason that passes (429, 5xx, overloaded, a lost connection, a stream that
  broke mid-reply) instead of ending the run: waits from 5 s, doubling, capped
  at a minute, up to 10 in a row, or with `--max-time` while a minute is left.
  Each retry is a warning and a `system/api_retry` event.
- `maxParallelAgents` (default 4) caps Task sub-agents running at once.
- A failed sub-agent's result starts with `FAILED:` and tells the model its
  task is not done, so it can't be reported as finished.

### Goals and side questions
- The `/goal` check treats the agent's own messages as claims: a `met` verdict
  must cite the tool output that proves it, failed tool calls and sub-agents
  are listed first so a long session can't hide them, and the final summary is
  marked as claims to check. It runs on the session's model (`goalCheckModel`
  picks another), not the small one.
- `/btw` mid-turn knows what is running: the tool calls (sub-agents included)
  that haven't returned, how long the turn has run, and the background tasks.
- On an OpenAI-compatible endpoint, WebFetch summaries and side requests use
  the session's model unless `smallFastModel` names one, instead of a model
  the endpoint may not serve.

### Fixes
- `/model` (and any picker or dialog taller than the input) no longer ends the
  session with "The cursor position could not be read": the terminal UI
  tracks its own position instead of asking the terminal while it reads keys.
- Injection notes: Forge's own Read notes ("Showing lines 1-2000 of N", "unchanged
  since you last read") are no longer flagged; a note appears once per file or
  page, reads calmly for local files, and isn't wrapped twice.
- After a denied tool call the model is told not to reach the same result another
  way (another command, shell or tool); after an environment failure, to stop
  and say what is missing. Replies are kept short for short messages.

### Terminal UI: Forge's look
- Forge's palette: every colour is a Pajamas stop from Forge for VS Code
  (brand purple, link blue, CI-style green, amber and red), with its own value
  for dark and light backgrounds. 24-bit where the terminal says it has it
  (`COLORTERM`, Windows Terminal, iTerm, WezTerm, VS Code), else the nearest of
  256, else the 16 ANSI colours; `FORGE_COLOR_DEPTH` overrides. The `auto`
  theme (now the default) follows `COLORFGBG`.
- Forge's F-and-cube logo opens a session, drawn in half blocks beside
  ForgeCLI's version, the model and where it runs, the directory and the keys
  to know, then Forge's welcome line. Narrow terminals get the small logo, or
  none; without colour it keeps its shape.
- With no endpoint set up, a first-run card (the logo, what ForgeCLI is, the
  servers it works with, the variables to set and what Forge found) replaces
  the bare error. It still exits with 3.
- The transcript is Forge's timeline: a tool call shows its turning blue dot
  while it runs, then goes to scrollback with a green or red dot and its
  result under `└`; paths are in link colour, commands follow a `$`. Answers
  have a quiet dot, prompts a purple `❯` on a raised surface; notices lead with
  `·`, `▲` or `✕`.
- The spinner speaks Forge (Forging, Hammering, Tempering…) with a shimmer
  sweeping the word; waiting for an answer is amber. The composer asks "What
  shall we forge today?". The status line starts with Forge's `▛◆` and shows a
  context meter (`━━━─────`) that turns amber at 70% and red at 90%.

### Terminal UI
- Results say what happened: `Read 40 lines`, `Updated calc.py: 1 addition,
  1 removal` with the changed lines in red and green, the first lines of a
  command's output. Permission dialogs for edits show the change; the spinner
  says `Waiting for your answer` while one is open.
- Tables are drawn with aligned columns; code blocks without their fences;
  wrapped lines continue under their text instead of at column 0.
- The header is one line (`ForgeCLI 0.1.0 · model · ~/project`); the cost reads
  `cost ?` from the start for a model without a known price.
- Two-column output (`/status`, `/doctor`, the shortcuts) wraps under its value
  column; a long path breaks in place instead of leaving a bare label.
  `/status` and `/doctor` show paths under the home directory as `~/…`.
- `/doctor` colours only its failed rows; an unknown model is one `note` row
  (guessed limits, no price) instead of two failures.
- The context meter follows each request of a turn, and reads `<1%` rather
  than `0%` once something is in the window. `/context` puts the measured size
  on its own line; screens with nothing to choose scroll without a pointer.
- A streaming answer is set apart from the prompt from its first words. Without
  colour (`NO_COLOR`), inline code keeps its backticks.
- HTTP errors read `API Error: HTTP 401 (…) from <url>: …` (no doubled "API
  error").
- `forge` opens a terminal UI when stdin and stdout are terminals: answers
  stream into the terminal's own scrollback, and a live region at the bottom
  holds the spinner, dialogs, queued messages, a multiline input box, the
  `/` command menu and a status line (mode, model, context, cost).
- Permission, question and plan-approval dialogs; Esc and Ctrl-C interrupt;
  Shift+Tab cycles the permission mode; messages typed during a turn are
  queued.
- Prompt history per project directory in `<state>/history.jsonl`; Ctrl+R
  searches it.
- Pickers for `/model`, `/resume`, `/rewind`, `/output-style` and
  `/permissions` typed without an argument; `@` completes project paths.
- UI-only commands: `/theme` (dark, light, none), `/copy [n]` (OSC 52),
  `/keybindings` (also `?`), `/statusline <command>` and `/terminal-setup`.
- `--no-tui` or `FORGE_TUI=0` keeps the line REPL.
- Forge's own marks: a braille spinner (`⠋⠙⠹…`), `•` before answers, `›`
  before tool calls, `↳` before results, `»` and `‖` for the accept-edits and
  plan modes. `/export` and the line REPL use the same marks.
- `/model` lists only the models your endpoint reports (an OpenAI-compatible
  `/models` listing); with the default backend it shows the current model and
  takes any id. Messages name models by id.

### Slash commands
- One command registry for every mode: `/help`, `/status`, `/usage` (`/cost`,
  `/stats`), `/doctor`, `/hooks`, `/memory`, `/skills`, `/agents`,
  `/plugin`, `/mcp`, `/tasks` (`/bashes`), `/release-notes`, `/clear`,
  `/compact` and `/exit`. Unknown commands report `Unknown command: /name`,
  and skills chain (`/a /b text`).
- Settings commands that apply at once: `/model`, `/effort`, `/fast`,
  `/config key=value` (`/settings`), `/output-style`, `/autocompact`,
  `/sandbox`, `/permissions` (`/allowed-tools`) and `/add-dir`. In the REPL
  they also save the default; in `-p` they change only that run, except
  `/config`, which always saves and says which settings layer would override it.
- Session commands: `/rename` (Forge suggests a name), `/export`, `/diff`,
  `/context [all]` and `/debug`, which turns on a session debug log and
  has Forge read it.
- Session commands: `/rewind` (restore code, conversation or both, or
  summarize part of the conversation), `/branch`, `/resume`, `/cd`,
  `/reload-skills`, `/reload-plugins`; `/clear` now starts a new session
  and keeps the old one resumable.
- `/advisor <model>`: an Advisor tool the model can consult; it sees the
  conversation so far, and its cost counts toward the session.
- `/import`: MCP servers and instructions from Codex, Gemini CLI and
  Cursor, planned first and applied with `--yes`, following the MCP trust
  rules.
- `/subtask <task>`: a background agent forked from the conversation (C20).
  Its report comes back with the next prompt; `-p` waits for it.
- `/mcp reconnect|enable|disable <server|all>`: restart a server, or turn it
  off and on. Disabling hides its tools and prompts at once and is saved in
  local settings; stream-json hosts get the same through `mcp_reconnect` and
  `mcp_toggle`.
- `/feedback` (`/bug`, `/share`): a private bug-report bundle on this
  machine, with secrets masked, including those inside free text. Nothing
  is uploaded.
- Bundled skills, in Forge's own words: `/init`, `/code-review` (`/review`),
  `/security-review`, `/simplify`, `/verify`, `/run`, `/run-skill-generator`,
  `/batch`, `/fewer-permission-prompts`, `/update-config`.
- Scheduled prompts (C19): `/loop` with an interval or self-paced, and the
  `CronCreate`, `CronList`, `CronDelete` and `ScheduleWakeup` tools. `-p`
  keeps running while tasks are pending.
- `/goal`: Forge keeps working until a small-model check finds the goal
  met, with guards against loops (C18). `/plan`, `/btw` side questions,
  `/recap`, and `!command` shell mode.
- Fast mode on models that offer it (`fastMode` setting, `/fast`).
- Custom commands, skills, output styles and plugins (`--plugin-dir`).

### `@` mentions
- `@path` in any prompt (TUI, REPL, `-p`, stream-json) attaches the file:
  the prompt stays as typed, and the contents follow in a system reminder.
  `@"quoted paths"`, `~/`, directory listings, and notes for images, PDFs,
  binaries and files the permission rules keep out. Limits: 50,000 characters
  per file, 10 files and 200,000 characters per message. Attached files count
  as read, so the model can edit them at once.
- One implementation (`forge_agents::attach`) serves prompts and custom
  commands. UserPromptSubmit hooks see the prompt without the attachments.
- `/loop` and scheduled prompts attach their `@` mentions too, read afresh
  on each run.
- An attached file can't close its own wrapper, and text that looks like
  instructions to an agent is flagged as data, as for Read output.
- TUI: completion quotes paths with spaces, and a dim `(attached: ...)` line
  follows the prompt.

### Fixes from checking the four together
- A screen that arrives while a permission prompt is open (`/context` typed
  mid-turn) waits for the answer instead of replacing the prompt, which
  denied the tool and interrupted the turn.
- The TUI's `/` menu follows a reload that keeps the session
  (`/reload-skills`, `/hooks add`, `/agents create`): new skills show up.

### The manual check run
- `docs/CHECKLIST.md` is one ordered run of 55 checks in three parts (print
  mode, interactive, special setups), with exact commands, what to see,
  where to look and a results table. The old numbers stay in brackets.
- `scripts/check-setup.sh [dir] [--force]` builds the scratch repository it
  uses.
- Tests keep it honest: every `forge ...` command in it parses, every slash
  command exists, and the print-mode checks a mock can stand in for are run.
- `docs/BASELINE.md` explains how to record the first real `forge-eval`
  baseline; a test checks its commands.

### TUI screens
- Screens: a command typed without arguments can open a scrolling viewer in
  the live region (still an inline viewport, no alternate screen). Up/Down,
  PageUp/PageDown, Home/End scroll; Enter acts on a row; Esc goes back.
- `/diff` lists the changed files with their +/- counts; Enter jumps to a
  file's hunks, coloured by side.
- `/context` is a 10×10 grid coloured by what fills the window, with the
  same numbers as the text form; it opens mid-turn too.
- `/hooks add <Event> <matcher> <command> [--scope ..]` and
  `/hooks remove <Event> <n>` change hooks in settings and reload the
  session; `/hooks` numbers each event's hooks and names their source. The
  TUI's hooks screen adds (a form) and removes (Enter, then confirm).
- `/agents create <name> --description .. [--tools ..] [--model ..]
  [--scope project|user]` writes an agent definition and reloads; the TUI's
  agents screen has a wizard for it.
- Key rebinding: `<config>/keybindings.json` maps keys to named actions
  (`submit`, `historySearch`, `cycleMode`, ...; `none` unbinds).
  `/keybindings` shows the keys in effect; bad entries are warnings.
- `/color <name>` sets the accent colour for this session; `/focus` keeps
  tool calls out of the scrollback until turned off.

### Immediate commands mid-turn
- `/status`, `/usage` (`/cost`, `/stats`), `/tasks`, `/context`, `/mcp` and
  `/btw` answer at once while a turn runs, in the TUI and for stream-json
  hosts, instead of waiting for the turn to end. The spinner keeps going.
- They read a session view the engine and driver publish (after each model
  call and tool batch, and when idle), so mid-turn numbers are current:
  `/usage` counts the turn so far.
- Stream-json marks their `result` lines `"immediate": true`, so hosts can
  tell them from the running turn's result.
- `/btw` mid-turn answers from the conversation so far; its cost and
  exchange, and `/mcp`'s refresh of instructions and `system/init`, are
  applied when the turn ends. A second `/btw` in the same turn sees the
  first.

### Fixes from the front-end review
- A prompt that starts with a file name (`/package.json has the wrong
  version`) is a prompt, not `Unknown command`.
- A stream-json host's `set_model` now reaches the system prompt too, which
  told the new model it was the old one.
- A host's `set_permission_mode: bypassPermissions` is refused unless the
  session was launched in that mode or with
  `--allow-dangerously-skip-permissions`, and always when managed settings
  set `disableBypassPermissionsMode`.
- The REPL said nothing when `--max-turns`, `--max-budget-usd` or a failed
  compaction stopped a turn: these now arrive as error notices, like API
  errors (print mode still reports them once, with the result).
- Secret masking (`forge config`, `/feedback`) now also catches a bearer
  token inside a value whose key looks harmless (custom headers), quoted
  keys in JSON or YAML text (`"api_key": "..."`), private key blocks, and
  more token shapes (Google, npm, GitLab, Stripe, Slack). The `/feedback`
  report and doctor files are masked too.
- `/import codex` no longer fails on a `config.toml` with several `[[x]]`
  entries that share a key.
- An advisor model priced through `modelPricing` counted as free against
  `--max-budget-usd`; its requests are now priced like the session's own.
- A subtask's edits made while a later prompt ran were checkpointed under
  that prompt: `/rewind <that prompt> code` undid them, and the verification
  loop counted them as the prompt's own writes. They now keep their own turn.
- `/tasks` lists subtasks that have finished (the last 20), not only the
  running ones: they were handed back before the list was made.
- In the terminal UI, `/keybindings` and `/terminal-setup` answer at once
  while a turn runs instead of waiting in the queue.
- Ctrl-D during a turn in the terminal UI interrupts it before leaving,
  instead of leaving it running while Forge waits to exit.

### Fixes from the slash-command review
- Scheduling: cron times no longer hang or skip a day across daylight-saving
  changes; the scheduler follows the wall clock after the machine sleeps;
  `/loop 50m` rounds to hourly and `/loop 20h` to daily; a `-p` run stopped
  by a limit while tasks are scheduled exits 4; after stdin ends, host
  permission prompts are refused instead of hanging.
- Goals: Ctrl-C during the goal check stops the loop; a hook's
  `continue: false` pauses it; `--max-turns` and `--max-budget-usd` count
  across the loop, and the check's cost is counted; a first-call credential
  error clears the goal; pauses are reported as `system/goal` events.
- Sessions: `/reload-*`, `/branch` and `/cd` keep the goal, scheduled tasks,
  background shells and the sandbox; rules allowed for the session survive
  `/clear`; `/cd` brings the new directory's memory files; forks keep their
  file checkpoints; `/rewind` skips synthetic messages and finds checkpoints
  for prompts that changed nothing; `--resume` of another directory's session
  is refused (use `--fork-session`); a torn transcript line no longer makes a
  session unresumable; SessionStart runs with source `resume`.
- Settings: a settings file Forge can't parse is never overwritten, and
  writes are atomic; `/permissions remove` can't drop managed or
  command-line rules; `/config` reports its notes and applies
  `verification.enabled` at once; `.forge/settings.local.json` is kept out
  of git; a custom `anthropic-beta` header joins the beta list; fast mode is
  priced at its rate.
- `!command` runs only in the interactive session (on `-p` it is an ordinary
  prompt), can be interrupted, and keeps at most 8 MB of output.

### Agent harness
- Long commands are no longer killed at the Bash timeout (C22). A build,
  install or test run still going after the call's wait (2 minutes by
  default) moves to the background; the call returns its ID and the output so
  far, and the model is told when it exits. A turn about to end waits for such
  commands (up to 10 minutes), so the result isn't lost and the command isn't
  run twice. Background shells started with `run_in_background` are reported
  when they exit too. Before, a wrong guess at the timeout killed the
  command, could leave a half-installed environment, and cost the whole run
  again.
- Prompt: what the user specifies (names, paths, formats) is a requirement;
  no stubs, mock data or hard-coded answers in place of the real thing; fix a
  failing tool's cause, not its source or installed packages; learn what an
  existing thing does before reimplementing it; checks are run again only
  after another change; one-sentence updates between tool calls.
- Verification loop: changes don't end a turn until checks have run (C12).
- Recovery:
  - Edit "did you mean" hints;
  - a loop guard;
  - continuation after `max_tokens`, with a higher output cap (C13);
  - tool calls the model ended itself with invalid JSON arguments (a missing
    comma, quote or brace, raw newlines in a string) are repaired when a small
    fix is safe (`system/tool_input_repaired`); otherwise the error says the
    JSON was invalid and shows where, instead of blaming the output limit.
- Safety:
  - dangerous-command patterns always ask;
  - injected instructions in tool output are marked as data (C14);
  - an OS sandbox for Bash (`--sandbox`).
- Context:
  - long output is saved to a file and the result points to it;
  - no repeated reads;
  - the task list survives compaction and resume (C15);
  - an agent can set its own `effort` (frontmatter or `--agents` JSON); the
    built-in Explore agent searches at `low` effort, so exploration costs
    less. Models without effort levels ignore it.

### Measurement
- `--autonomous` (`FORGE_AUTONOMOUS=1`, setting `autonomous`), for runs nobody
  watches:
  - no question or plan-approval tools;
  - a "Running unattended" prompt section;
  - one reminder to make a real attempt when a turn would end without any tool
    call;
  - two verification reminders.
  The Harbor agent turns it on; `FORGE_HARBOR_AUTONOMOUS=0` turns it off.
- `--max-time` (`900`, `15m`, `1h`): the model is told its time limit up
  front and reminded to wrap up when a fifth of it is left. The Harbor agent
  passes each Terminal-Bench task's own limit, so runs end with a report
  instead of being cut off mid-change.
- The Harbor agent can trust extra certificate authorities in task containers
  (`FORGE_HARBOR_CA_BUNDLE`), for networks that inspect TLS; such jobs are
  named `-hostca` and are for local comparisons only.
- `evals/harbor/report.py` writes `jobs/<job>/report.md` when a job ends:
  setup, score, a verdict per task (passed, false success, gave up, refused,
  API error, timeout, ...), signals (429s, retries, tool errors by kind,
  reminders), and per task the instruction, verifier failures and the whole
  conversation. Several jobs also give a side-by-side comparison. The agents
  save the instruction and their settings for it; keys are removed.
- `evals/harbor/run.sh` rebuilds the static binary when the code changed
  since it was built, so a job named after a commit runs that commit's code.
  Jobs labelled 02195fd probably ran an older binary. `FORGE_STATIC_BIN`
  jobs are named `-custombin`.
- `evals/harbor/capture-prompts.sh` saves the system prompt and tool
  descriptions that ForgeCLI, Forge Code and Ante send, through a fake
  endpoint on localhost, into `evals/harbor/prompts/`. Captured so far:
  ForgeCLI (1056 words, 19 tools) and Forge Code 2.13.21 (1660 words, 13
  tools).
- `evals/harbor/watch.py` shows a run like a conversation: what the model
  says, one short line per tool call, failed calls with their error, how the
  run ended and why, and each task's reward. `--thinking` adds the reasoning,
  `--full` the tool input and output. Other agents' logs are followed too.
  A multi-line command shows the model's description of it, and a task that
  starts after `watch.py` is shown from its first step.
- `evals/harbor/ante_agent.py` and `run-ante.sh` run Ante on the same tasks,
  for side-by-side comparisons; `forgecode_agent.py` and `run-forgecode.sh` do
  the same for Forge Code (forgecode.dev), downloading its static Linux build.
  Ante runs with `--yolo --no-session-save --no-skills`, as Ante's own Harbor
  agent does (`ANTE_HARBOR_ARGS` overrides).
  `FORGE_HARBOR_MODEL` sets the model for all three run scripts (default
  `deep-thinking`; a `provider/` prefix is optional).
- The Harbor agent rewrites a `localhost` endpoint to this machine's address
  (`FORGE_HARBOR_HOST_IP` to set it), so self-hosted models can be used. A
  login in the URL is kept, and when the address can't be found the error
  says to set `FORGE_HARBOR_HOST_IP`.
- `evals/harbor/`: a Harbor agent that runs ForgeCLI on Terminal-Bench, with
  a static (musl) build script and a run script for a 10-task subset or the
  full 5-attempt run, each job named after its commit.
- The `slugify-trap` eval now checks that behaviour is unchanged, as its
  instruction says.

### Tools and integrations
- MCP client (stdio, streamable HTTP, SSE), `forge mcp` and `forge mcp serve` (C16).
- WebFetch (small-model answers), WebSearch, AskUserQuestion, plan mode, `--worktree`.
- Sub-agents (the Task tool) with the built-in Explore and Plan agents, plus custom agents.

### Core
- Headless print mode with text, json and stream-json output, and the SDK
  control protocol.
- Sessions:
  - JSONL transcripts;
  - resume and fork;
  - file checkpoints and rewind;
  - compaction.
- `forge-eval`, a task suite measuring pass rate, cost, turns, recovery and
  false finishes.
