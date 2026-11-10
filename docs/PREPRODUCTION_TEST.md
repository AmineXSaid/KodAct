# Pre-production test prompt for the KodAct terminal

Give the prompt below to a coding agent that has this repository checked out
and can run commands. It tells the agent to drive the real `kodact` binary
the way a person does, find every fault a person would meet, fix each one in
the mechanism (never in the test), and prove the fix.

It exists because of a fault found by use and missed by the tests: `/clear`
answered "Conversation cleared" and left the old conversation on screen. The
unit tests checked the answer's text; nobody checked the screen.

---

## The prompt

You are the release gate for KodAct's terminal. Your job is to make it safe
for people to use in production: find every fault a person would meet at the
keyboard, fix each one, and prove each fix. Read `KODACT.md`, `docs/GOALS.md`,
`docs/goal.md`, `docs/TUI.md`, `docs/CLI.md` and `docs/PARITY.md` before you
start, and follow the rules in `KODACT.md` for every change.

### Rules that decide whether your work counts

1. **Test the real binary, on a real terminal.** Build `kodact` and drive it
   through a pseudo-terminal: send keystrokes, then read the *screen* with a
   terminal emulator (for example Python `pty` plus `pyte`). A unit test
   passing is not evidence. The text of an answer is not evidence. Only what
   is on the screen, in the files and in the session store after the action
   counts.
2. **Check the state, not the message.** For every action ask three
   questions. Did it say it worked? Did the screen change the way a person
   expects? Did the underlying state (the conversation, settings files,
   session store, working directory, git tree) change to match? A command
   that says "done" while the screen or the state disagrees is a bug.
3. **No model is needed, and none may be faked silently.** Run a local stub
   endpoint that streams scripted replies, including tool calls, slow
   replies, errors (401, 429, 500, a dropped connection, malformed stream)
   and a reply longer than the screen. Say in your report what was stubbed.
4. **Fix the mechanism.** When you find a fault, look for its siblings before
   you fix it: the `/clear` fault was also in `/resume`, `/rewind` and
   `--continue`. Fix the cause once. Never weaken, skip or delete a test to
   get green, and never special-case the input that showed the fault.
5. **Every fix gets a test that fails without it**, at the level where the
   fault lived (the session's events, the screen buffer, the file on disk).
   Then run the gates: `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets` with no warnings, and
   `cargo test --workspace`.
6. **Keep a ledger** as you go: each check, what you did, what you saw, pass
   or fail, and for a failure the cause and the fix. Report faithfully. A
   check you could not run is listed as not run, with the reason. Do not
   write "works" for anything you did not see work.
7. **Do not stop at the first pass.** After fixing, run the whole matrix
   again on the new build; a fix can break a neighbour.

### The matrix

Run everything below in **both displays** (the full-screen UI and
`KODACT_TUI=inline`) unless a row says otherwise, and repeat the rows marked
(sizes) at 200x50, 110x36, 80x24, 60x20 and 40x12 columns by rows, and after
resizing the window while the thing is on screen.

**A. Start and first run**
- No endpoint set: the first-run card shows, names what is missing, exits
  with the documented code, and nothing is written outside the config and
  state folders.
- Each provider path starts: `KODACT_BASE_URL`, `KODACT_OPENAI_BASE_URL`,
  Gemini. A wrong key, a wrong URL, an unreachable host and a model the
  endpoint does not list each give a message a person can act on, and the
  UI stays usable.
- `kodact "prompt"`, `kodact -p`, `--continue`, `--resume <id>`,
  `--resume` with an unknown id, `--no-tui`, `KODACT_TUI=0`, `--model`,
  `--permission-mode` for every mode, `--add-dir`, `--no-input`. After
  `--continue` and `--resume` the earlier conversation is on screen.
- The front page (sizes): each style (`frame`, `glow`, `card`, `ledger`,
  `status`, `bare`), each colour setting (`dark`, `light`, `none`, `auto`),
  each colour depth (`KODACT_COLOR_DEPTH=truecolor|256|16`), `NO_COLOR`,
  with and without earlier conversations, with an MCP server that failed.
  Nothing overflows, wraps badly or overlaps at any size.

**B. Every slash command, every form**
Take the list from `kodact_core::commands::BUILTINS`, not from memory, so a
command added later is covered. For each command and each alias:
- With no arguments, with each documented argument, with a wrong argument,
  with too many arguments, with leading and trailing spaces, and in upper
  case.
- Typed after a turn, on the empty front page, while a turn is running, and
  while a dialog or picker is open.
- From the menu (type `/`, move with the arrows, Tab to complete, Enter),
  and typed in full.
- If it opens a picker or a screen: choose each kind of row, filter by
  typing, page with PgUp and PgDn, cancel with Esc, and confirm that Esc
  leaves everything as it was.
- If it changes something that lasts (settings, permissions, hooks, the
  theme, the model, the status line): confirm the file on disk, quit, start
  again and confirm it held. Then undo it and confirm that too.
- If it changes the conversation (`/clear`, `/resume`, `/rewind`, `/branch`,
  `/compact`, `/cd`): confirm the screen shows the conversation the session
  now has, the context meter and token count match, the next prompt goes to
  the right conversation, and the conversation left behind can be reached
  again with its content whole.
- `/exit` and `/quit` leave the terminal as they found it: cursor visible,
  no mouse capture, no alternate screen, no raw mode, the transcript in the
  scrollback, the resume hint printed.
- An unknown command, a command of only `/`, and a skill or custom command
  whose name matches a built-in.

**C. The editor and the keys**
- Every key in `/keybindings`, one by one, and every rebinding in
  `keybindings.json`, including a broken file.
- Typing: multi-line input by every documented route, paste of 1, 100 and
  10,000 lines, paste containing tabs, ANSI escapes and a trailing newline,
  wide characters (CJK), combining marks, emoji with joiners, right-to-left
  text, and a line longer than the window. The cursor is always where the
  next character lands.
- History: Up and Down, Ctrl+R search, history kept per directory, a prompt
  with newlines recalled whole.
- `@` file mentions: the menu, filtering, a path with spaces, a file that
  does not exist, a project with many thousands of files.
- `!` shell mode: a command that succeeds, fails, prints a megabyte, prints
  colour, waits for input, and runs for a minute and is interrupted.
- Esc, Esc twice, Ctrl+C once and twice, Ctrl+D, Ctrl+L and Shift+Tab, each
  on an empty prompt, with text typed, during a turn, and with a dialog
  open.

**D. A turn, start to finish**
- A text answer shorter and longer than the window; scroll back with PgUp
  and the wheel while it streams, and confirm the view stays put, then
  returns to the newest line on send.
- Markdown of every kind the renderer handles: headings at each level,
  nested lists, numbered lists, quotes, tables wider than the window,
  inline code, fenced code in each highlighted language, a `diff` fence,
  links, bold and italic, and malformed markdown (an unclosed fence, an
  unclosed bold).
- Tool calls: each built-in tool once succeeding and once failing; several
  in one reply; a tool whose result is empty, huge, binary, or not UTF-8;
  a sub-agent; a background shell command.
- Permission prompts: allow, deny, allow always, each suggestion offered;
  in each permission mode; with a rule from `/permissions` already
  matching; two prompts arriving close together; a prompt answered after a
  resize.
- Plan mode: enter, get a plan, approve, reject, edit.
- Interrupt: Esc during the answer, during a tool call, during a permission
  prompt, and during a retry wait. After each, the UI is idle, nothing
  shows as still running, and the next prompt works.
- Messages typed during a turn are queued, shown as queued, sent in order,
  and can be withdrawn.
- Errors from the endpoint (each from rule 3) show a message a person can
  act on, say whether a retry is under way, and leave the session usable.
- Thinking, where the stub sends it: shown while it streams, folded after.

**E. Long sessions and limits**
- A session of 200 turns: memory stays level, typing stays responsive,
  scrolling to the top and back works, the context meter is right.
- Auto-compaction on a small window (`KODACT_CONTEXT_WINDOW=20000`) and on a
  large one (1,000,000): it happens at the stated point, says so, and the
  conversation continues with its facts intact. `/compact` by hand, with
  and without instructions, and on an empty conversation.
- `/rewind` in each of its modes after edits to files: the files and the
  conversation both go back, and the screen shows what is left.
- `/branch`, then work in each branch, then `/resume` between them.
- `/loop`, `/tasks`, `/subtask`, `/goal`: start, list, let one finish, stop
  one, and `/clear` while one runs.

**F. The world around it**
- Sessions: two `kodact` processes in one directory at once; a session
  store that is read-only, full, or holds a corrupt file; a session resumed
  from another directory.
- Settings: each layer (user, project, local), a setting given in two
  layers, a settings file with a syntax error, and one changed by another
  process while the UI runs.
- Hooks: one that passes, one that blocks, one that hangs, one that prints
  a megabyte.
- MCP: a server that works, one that fails to start, one that dies
  mid-session, `/mcp reconnect`, enable and disable.
- Skills, commands, agents and plugins from `.kodact/` and from the
  reference CLI's folders (see "Naming" in `KODACT.md`), and
  `/reload-skills` after changing them.
- The terminal: `TERM=dumb`, `TERM=xterm`, inside tmux and screen, over
  SSH, with stdin or stdout not a terminal, and the window closed or the
  process sent SIGTERM, SIGHUP and SIGINT mid-turn. In every case the
  terminal is left usable and the conversation can be resumed.
- Suspend with Ctrl+Z and resume with `fg`.
- Platforms: run what you can on Linux, macOS and Windows; list what you
  could not run.

**G. Safety**
- Nothing secret is printed: keys are masked in `/status`, `/doctor`,
  `/config`, `/debug` logs, `/feedback` bundles and `/export`.
- Paths outside the working directories are refused where the docs say so,
  including through symlinks and `..`.
- The sandbox settings do what they say for shell commands.
- `--dangerously-skip-permissions` is the only way past a prompt, and
  `--no-input` denies every prompt.
- Files `kodact` creates have the documented permissions.

### What to hand back

1. The ledger: every row of the matrix with pass, fail (fixed), fail (not
   fixed, with the reason) or not run (with the reason).
2. For every fault: how to reproduce it by hand in three lines or fewer,
   the cause, the fix, its siblings you checked, and the test that now
   guards it.
3. The harness you built to drive the terminal, committed under `scripts/`
   or as tests, so the next release runs the same matrix with one command.
4. The gates' output on the final build.
5. A plain verdict: is this build safe to release? If not, the shortest
   list of what stands in the way.
