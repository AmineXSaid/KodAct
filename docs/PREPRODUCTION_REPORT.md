# Pre-production report: the terminal

The run of `docs/PREPRODUCTION_TEST.md` on 2026-11-10 and 11, on Linux
(x86-64, 4 cores), against a debug build of this branch.

## Verdict

**Safe to release on Linux, in the full-screen display, with the three
caveats below.** The matrix found 22 faults a person would meet; all 22 are
fixed in the mechanism and each has a test that fails without the fix. The
final build passes the whole matrix (7,004 rows pass, 0 fail, 1 known and not fixed, 21 not run) and the three gates.

What stands between this build and "safe everywhere":

1. **macOS and Windows were not run.** Nothing here says the fixes hold
   there; the signal handling is Unix-only code, and the Windows terminal
   was not driven at all. Run the matrix on each before releasing for it.
2. **The OS sandbox was not run** (no `bwrap` on the machine). What was
   checked is that without a backend a `--sandbox` command is never run
   unconfined.
3. **Two permission changes ask where KodAct used to run** (a read-only
   shell command that names a path outside the working directories; a read
   through a symlink that leads outside). They close real holes, and they
   change behaviour people will notice. They touch the harness, so the
   `kodact-eval` before/after comparison `KODACT.md` asks for is **owed**:
   no API key was available in this run.

One fault is known and left (inline display, 12-row window; below), and one
thing was seen once and not again (below).

## What was stubbed

- **The model.** A local OpenAI-compatible endpoint (`scripts/terminal-matrix/kterm.py`,
  `Stub`) streams scripted replies: text, tool calls, thinking, slow
  replies, HTTP 400, 401, 403, 429 and 500, a dropped connection, a malformed
  stream. It also answers the Messages API and Gemini paths with text.
  No real model ran, so nothing here measures task success.
- **The terminal** is pyte, a terminal emulator, on a real pseudo-terminal.
  It does not draw joined emoji as one glyph (those cursor rows are "not
  run") and has no alternate screen of its own (the modes KodAct leaves on
  are read from the escape sequences it wrote). tmux was driven for real.
- **The keyboard protocol**: the harness answers KodAct's query as kitty
  does, so Shift+Enter arrives as its own key. No real kitty, foot or
  WezTerm was used.
- An MCP server (`mcp_stub.py`): a real process speaking the protocol.

Everything else is the real thing: the binary, its sessions, settings,
hooks, shell, permission engine and MCP client.

## The ledger

The full ledger is written by the run to `target/terminal-matrix/ledger.md`
(one row per thing checked). By section, on the final build:

| Section | Checks | Passed | Failed | Known, not fixed | Not run |
| --- | ---: | ---: | ---: | ---: | ---: |
| A. Start and first run | 19 | 680 | 0 | 0 | 0 |
| B. Every slash command, every form | 118 | 4358 | 0 | 0 | 2 |
| C. The editor and the keys | 16 | 530 | 0 | 0 | 8 |
| D. A turn, start to finish | 20 | 897 | 0 | 1 | 0 |
| E. Long sessions and limits | 8 | 74 | 0 | 0 | 0 |
| F. The world around it | 22 | 337 | 0 | 0 | 10 |
| G. Safety | 8 | 128 | 0 | 0 | 1 |
| **All** | 211 | **7004** | **0** | **1** | **21** |

"Not run" rows, with their reasons:

- B: /sandbox read-only saved and held: no sandbox backend (bwrap) on this machine: /sandbox refuses to turn on, and says so
- C: emoji with joiners: this emulator (pyte) draws joined emoji as separate glyphs; a real terminal's width rule decides
- F: a read-only session store: running as root: a read-only folder is still writable, so chmod cannot make the store read-only
- F: a session store on a full disk: needs a size-limited filesystem (a mount); the unwritable-store row covers the same write failures
- F: the whole check: GNU screen is not installed on this machine
- F: macOS: no macOS machine in this run
- F: Windows: no Windows machine in this run
- F: over SSH: no SSH server in this run; the pseudo-terminal checks cover what SSH gives the program (a pty, TERM, no local display)
- F: resume with fg: Ctrl+Z is not a suspend key in the UI (raw mode takes it); there is nothing to resume
- G: the sandbox confining a command (read-only and workspace-write): bubblewrap (bwrap) is not installed on this machine

Parts of the matrix no check was written for (**not run**):

- Wrong key, wrong URL, unreachable host and unlisted model were checked on
  the OpenAI-compatible path only; the Messages and Gemini paths were
  checked for starting and answering.
- `AskUserQuestion` (the question dialog and its typed answer), `WebSearch`,
  `BashOutput` and `KillShell` as tools called by the model; editing a plan
  before approving it.
- Hooks other than `PreToolUse`; hook events for sessions and compaction.
- A session store on a full disk (needs a size-limited filesystem); a
  read-only store (the run was root, for whom read-only folders are
  writable). The unwritable-store check covers the same failure to write.
- Over SSH; inside GNU screen (not installed).
- The line prompt (`--no-tui`) and `-p` print a model's or a tool's text as
  it is: whether control characters in it should be shown as pictures there
  too was not checked. The fix below covers the terminal UI.

## The faults

Each: how to see it by hand, the cause, the fix, the siblings checked, and
the test that guards it. "Matrix" names the check in
`scripts/terminal-matrix/checks/`.

### 1. A paste can command the terminal
- **By hand:** copy text containing `ESC [ 2 J` (or a tab) and paste it into
  the prompt. The screen clears; with a tab the input line is left in pieces.
- **Cause:** the prompt's text was written to the terminal as it was. Only
  lines that went through `text::wrap` had their control characters removed.
- **Fix:** `safe_cells` in `tui/mod.rs`: every cell either display writes
  has its control characters replaced by their pictures; `render::input`
  counts rows and cursor on what is drawn. What is sent is unchanged.
- **Siblings:** the status command's output, menus, dialogs, the sidebar,
  the front page, lines written above the inline viewport: all go through
  the same two backends now. Tool and shell output already went through
  `wrap`.
- **Test:** `tui::tests::no_control_character_reaches_the_terminal`; matrix
  C "paste".

### 2. With the sidebar open, new prompts and answers do not appear
- **By hand:** a window of 140+ columns, send a prompt, press Ctrl+B, send
  another. The second prompt and its answer never show (the request is
  sent and answered).
- **Cause:** `Full::commit` wrapped new lines for the window's width and
  added them only when that equalled the width the transcript was drawn at;
  beside the sidebar it never does.
- **Fix:** new lines are wrapped at the width the transcript is drawn at.
- **Siblings:** the scroll position while scrolled up used the same wrong
  width; fixed with it.
- **Test:** `tui::tests::new_lines_show_with_the_sidebar_open`; matrix C
  "keybindings.json" (the row that found it).

### 3. `kill` leaves the terminal unusable
- **By hand:** start `kodact`, send a prompt, `kill <pid>` from another
  terminal. The first terminal stays in raw mode on the alternate screen
  with the mouse captured.
- **Cause:** SIGTERM, SIGHUP and SIGINT had their default action.
- **Fix:** `StopSignals` in the UI loop: a signal interrupts the turn and
  ends the loop as `/exit` does; exit status 128 plus the signal.
- **Siblings:** a closed window (already ended cleanly); the resume hint
  used `eprintln!`, which panics when the terminal is gone; `-p` and the
  line prompt set no terminal modes.
- **Test:** `tui::tests::stop_signals_are_caught_not_fatal`,
  `a_signal_ends_the_ui_with_the_shells_status_for_it`; matrix F "SIGTERM,
  SIGHUP, SIGINT".

### 4. A picker longer than the window loses its title and filter
- **By hand:** an endpoint that lists 40 models, a 36-row window, `/model`.
  The list runs off the top; the title, the filter row and the first rows
  are gone, and PgUp/PgDn do nothing.
- **Cause:** a dialog drew every option, whatever the room.
- **Fix:** `render::dialog` keeps the title, filter and keys and scrolls
  the options around the selected one; PgUp, PgDn, Home, End move in every
  picker and question.
- **Siblings:** permission and plan dialogs at 40x12 (a tall question gives
  up the first rows of what it asks about, never its choices); every style.
- **Test:** `tui::render::tests::a_dialog_longer_than_the_window_scrolls_its_options`;
  matrix B "pickers".

### 5. The inline display breaks when the window is made smaller
- **By hand:** `KODACT_TUI=inline kodact`, shrink the window to 40x12. The
  input box, its border and the status line are all on the last row.
- **Cause:** the viewport is placed from a remembered cursor row; after a
  resize the terminal has moved the rows and cannot be asked where.
- **Fix:** `Screen::resized`: the window (not its scrollback) is cleared,
  the viewport restarts at the top, the newest lines that fit are rewritten.
- **Siblings:** growing the window; a viewport taller than the new window;
  `/clear` then a resize. The full-screen display was not affected.
- **Test:** `tui::tests::the_inline_viewport_is_whole_after_the_window_changes_size`;
  matrix A "front page: every style at every size".

### 6. `?`, `/status` or `/help` on the front page show only their last rows
- **By hand:** start `kodact`, press `?`. Six rows of the key table show.
- **Cause:** the front page treated every transcript row as a note and
  showed the last six.
- **Fix:** an answer longer than six rows, or than the room, takes the
  page's place in the conversation's layout, whole and scrollable.
- **Siblings:** the docked styles (ledger, bare); short windows.
- **Test:** `tui::render::tests::an_answer_on_the_front_page_is_shown_whole`;
  matrix C "every key".

### 7. A queued message cannot be withdrawn
- **By hand:** during a turn type a message and Enter. There was no way to
  take it back.
- **Fix:** Up on an empty prompt puts the newest queued message back in the
  input; the status line says so while one is queued.
- **Test:** `tui::app::tests` (the queue test); matrix D "messages typed
  during a turn".

### 8. PgUp skips rows in a short window
- **By hand:** a 40x12 window, a long answer, PgUp. Two rows in six are
  never shown.
- **Cause:** a page was half the window, more than the transcript rows in view.
- **Fix:** `Full::page`: never more than the rows in view less one.
- **Test:** `tui::tests::paging_never_skips_a_row_in_a_short_window`; matrix
  D "markdown" at 40x12.

### 9. `theme: none` still draws colour at start
- **By hand:** `{"theme": "none"}` in user settings, `KODACT_TUI=inline kodact`.
  The banner is in colour, for good.
- **Cause:** the app was built in the default colours and learned the
  setting an event later.
- **Fix:** `starting_app` applies `theme` and `tuiStyle` before the first frame.
- **Siblings:** `tuiStyle` (same path); `/theme dark` afterwards still works.
- **Test:** `tui::tests::the_app_starts_in_the_saved_colours_and_style`;
  matrix A "front page: colours".

### 10. The status line keeps the branch it started with
- **By hand:** `/cd /tmp` from a git project: the status line still says `⎇ main`.
- **Cause:** the branch was read once at start.
- **Fix:** each status snapshot reads it where the session is.
- **Siblings:** a branch switched by a command during the session.
- **Test:** `tui::session::tests::the_status_names_the_branch_where_the_session_is_now`;
  matrix B "/cd".

### 11. `/keybindings` leaves rebound keys out
- **By hand:** `{"f9": "lineStart"}` in `keybindings.json`, `/keybindings`:
  the "start / end of line" row does not list f9.
- **Cause:** rows covering two actions kept their default text.
- **Fix:** every row lists each of its actions' keys.
- **Test:** `tui::app::tests::key_bindings_apply_and_show_in_the_key_table`.

### 12. The front page cuts its hints mid-word
- **By hand:** a 60-column window, first run: `/init writes KODA`.
- **Fix:** each suggestion shows whole or not at all; the failed-MCP line likewise.
- **Test:** `tui::front::tests::every_style_fits_and_says_what_the_session_has`.

### 13. `@` completion loses top-level files in a large project
- **By hand:** a project with a 25,000-file folder and `my notes.md` beside
  it: `@my` offers nothing.
- **Cause:** the walk stopped at its first 20,000 paths, folder by folder.
- **Fix:** the shallowest 20,000 are kept (up to 400,000 looked at).
- **Test:** `tui::tests::a_large_folder_does_not_push_shallow_files_out_of_at_completion`;
  matrix C "@ file mentions".

### 14. `/model one two three` is saved as a model
- **By hand:** `/model gpt 4o`. Every request fails, in this session and the
  next (it is saved as the default).
- **Fix:** more than one word is refused; an id the endpoint does not list
  still switches (a gateway may serve it) and says so.
- **Test:** `core::driver_tests::model_refuses_words_and_says_when_the_endpoint_does_not_list_the_id`;
  matrix B "/model: every form".

### 15. A dead MCP server stays "connected"
- **By hand:** kill an MCP server's process mid-session, call its tool, then
  `/mcp`: "connected, 2 tools".
- **Fix:** a call that finds the connection closed marks the server failed;
  its tools leave the list; the error names `/mcp reconnect <name>`.
- **Test:** `kodact-mcp` `client::a_server_that_dies_is_no_longer_connected`;
  matrix F "MCP".

### 16. A session store that cannot be written stops KodAct from starting
- **By hand:** make the state folder unwritable (or a file), `kodact`:
  "Not a directory (os error 20)", exit 3.
- **Fix:** the session starts unsaved and says so, naming the folder; no
  resume hint is printed for it.
- **Test:** `core::driver_tests::an_unwritable_store_starts_the_session_unsaved_with_a_warning`;
  matrix F "session store".

### 17. A plugin's command sends `${CLAUDE_PLUGIN_ROOT}` unfilled
- **Fix:** plugin commands fill the plugin's folder and the project's, as
  plugin skills do. PARITY: "Plugins".
- **Test:** `agents::commands::tests::a_plugins_command_knows_its_own_folder`;
  matrix F "skills, commands, agents and plugins".

### 18. Error messages that say what is not so
- A 429 that ended the turn said "KodAct waits and retries"; a 404 from a
  wrong base URL said the model was unavailable and pointed at `/model`.
- **Test:** `tui::app::tests::an_api_error_says_what_is_true_now`.

### 19. A symlink out of the project is read without asking
- **By hand:** `ln -s ~/.ssh project/keys`, have the model Read
  `keys/id_rsa`: no question, the content is returned.
- **Cause:** paths were compared as text.
- **Fix:** `paths::real`: a path is inside only if where it leads is.
- **Siblings:** deny and ask rules now also match a link's target; an allow
  rule on a folder does not cover a link's target elsewhere; the folders
  reads are allowed from; a project itself reached through a link; `@`
  mentions (same decision).
- **Test:** `permissions::tests::a_link_out_of_the_project_is_outside_it`;
  matrix G "paths outside".

### 20. A read-only shell command reads anything, in every mode
- **By hand:** `kodact --permission-mode plan`, have the model run
  `cat /etc/passwd`: it runs.
- **Cause:** a read-only command was allowed whatever it named.
- **Fix:** a command that names a path outside the working directories is
  asked about (refused in don't-ask mode and `-p`); an allow rule still allows.
- **Siblings:** accept-edits allowed `touch ~/.profile` (its own check
  looked only for ` /` and `..`); sandboxed commands.
- **Test:** `permissions::tests::a_command_that_names_a_path_outside_is_asked_about`;
  matrix G "paths outside".

### 21. Transcripts are readable by other users
- **Fix:** the state directory is 0700.
- **Test:** `e2e::the_state_directory_is_private_to_its_owner`; matrix G
  "files kodact creates".

### 22. `kodact -p --no-input "prompt"` fails
- **Cause:** the flag took an optional value, so it took the prompt.
- **Fix:** a value only with `=`.
- **Test:** `e2e::no_input_does_not_take_the_prompt_as_its_value`; matrix G
  "--no-input".

## Known and not fixed

- **Inline display, 12-row window:** when a turn ends the live region
  shrinks and stays where its top was, so blank rows are left under the
  input and the end of a long answer is a scroll away in the terminal's
  scrollback. Moving the region down means rewriting rows the terminal
  already holds, which would show twice in its scrollback. The full-screen
  display, the default, is not affected.
- **Ctrl+Z does not suspend.** In raw mode the key reaches KodAct, which
  ignores it: the UI keeps running, unharmed, and there is nothing to bring
  back with `fg`. The matrix asks for suspend and resume; adding it means
  leaving and re-entering the terminal's modes around a SIGTSTP, which was
  not built in this pass.
- **Seen once, not again:** in one run of `/plan: every form`, after `/plan`,
  Shift+Tab and a prompt, "Working… (0s)" stayed on screen for 15 seconds.
  Fifteen reruns and a stress check of 25 rounds per display (matrix E "mode
  changes and prompts in quick succession") have not shown it again. The
  machine was running two displays' checks and a build at the time; it may
  have been load. It is not explained.
- **A unit test failed once:** one `cargo test --workspace` run exited 101
  in `kodact-cli`'s `contract` tests and passed on every rerun; the failing
  test's name was not captured.

## What KODACT.md asks of each change

- **Real use:** each fault above is something a person meets at the
  terminal; none exists for a benchmark.
- **Fix, don't fit:** nothing here depends on a model or its window: the paging and dialog limits come from the window's rows, the
  `@` cap is a count of paths. On a large model every fix behaves the same.
- **Pillars:** compatibility (the terminal UI, plugins) for 1 to 14, 17, 18
  and 22; failure recovery for 15 and 16; tool design (permissions) for 19
  to 21.
- **Eval tasks and measurement:** the terminal matrix is the measure for the
  UI faults. For 19 and 20, which change what a model's tool calls are
  allowed to do, no `kodact-eval` task was added and the before/after run is
  **owed** (no API key).

## The gates on the final build

```text
$ cargo fmt --all --check
(no output: clean)
$ cargo clippy --workspace --all-targets
0 warnings, 0 errors
$ cargo test --workspace
exit 0: 657 passed, 0 failed, 3 ignored
$ scripts/terminal-matrix/run.py
7004 passed, 0 failed, 1 known and not fixed, 21 not run in 1791s.
```
