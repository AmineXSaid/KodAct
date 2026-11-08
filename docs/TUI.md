# The interactive terminal UI (M8)

`kodact` with no `-p` opens the interactive UI when stdin and stdout are both
terminals. `--no-tui` (or `KODACT_TUI=0`) keeps the line REPL
(`crates/kodact-cli/src/repl.rs`), and print mode is unchanged.

## Status

| Part | State | Where |
| --- | --- | --- |
| Prompt editor: multiline text, word moves, kill commands, history | done, tested | `crates/kodact-cli/src/tui/editor.rs` |
| Theme, one-line markdown, wrapping by display width | done, tested | `crates/kodact-cli/src/tui/text.rs` |
| UI state machine: keys, streaming into scrollback, tool lines, `/` menu, permission/question/plan dialogs, queued input, Esc/Ctrl-C/Shift+Tab | done, tested | `crates/kodact-cli/src/tui/app.rs` |
| Session task (owns the Driver), `TuiSink`, `TuiPrompter` | done, tested | `crates/kodact-cli/src/tui/session.rs` |
| Renderer for the live region (`LiveView`) | done, tested | `crates/kodact-cli/src/tui/render.rs` |
| Terminal loop, inline viewport, scrollback writes, panic-safe restore | done, tested (`Screen`), checked by hand | `crates/kodact-cli/src/tui/mod.rs` |
| Wiring into `main`, `--no-tui`, `KODACT_TUI`, persisted history | done | `crates/kodact-cli/src/main.rs`, `args.rs` |
| Pickers, Ctrl+R search, `@file` completion | done, tested | `crates/kodact-core/src/commands/picker.rs`, `tui/app.rs`, `tui/session.rs` |
| Screens: the scrolling viewer and forms; `/diff`, `/context`, `/hooks`, `/agents`; key rebinding; `/color`, `/focus` (Phase 4) | done, tested | `kodact-core` `commands/screens.rs`, `tui/app.rs` (`Viewer`, `Form`), `tui/render.rs` |
| UI-only commands (`/theme`, `/copy`, `/keybindings`, `/statusline`, `/terminal-setup`) | done, tested | `kodact-core` `commands/settings.rs` (`/theme`, `/statusline`), `tui/session.rs` (the others) |

Dependencies are in `crates/kodact-cli/Cargo.toml`:
- `ratatui` 0.29 (MIT);
- `crossterm` 0.28 with `event-stream` (MIT);
- `unicode-width` 0.2 (MIT or Apache-2.0);
- `futures` (the workspace's), for `EventStream`.

## Decision

The UI is an **inline viewport**, not an alternate screen:
- finished output (your prompts, answers, tool calls, notices) is written
  into the terminal's own scrollback, so scrolling, search, copy and the
  history after exit work as in any shell;
- only a small live region at the bottom is redrawn: the streaming answer's
  unfinished line, the spinner, a dialog when one is open, queued messages,
  the input box, the `/` menu and the status line.

This is how the reference CLI behaves, and it keeps the UI robust: nothing
on screen depends on a virtual scroll buffer, and a crash leaves readable
output behind. The live region grows and shrinks with what it shows.

## Architecture

```
 crossterm EventStream ──► UI task ──(ToSession)──► session task (owns Driver)
                           App      ◄──(UiEvent)───  TuiSink, TuiPrompter, results
                           Screen: live viewport draw + insert_before(scrollback)
```

### Session task (`tui/session.rs`)

```rust
pub enum ToSession { Input(String), Exit }

pub async fn run(mut driver: kodact_core::Driver,
                 mut rx: mpsc::UnboundedReceiver<ToSession>,
                 ui: mpsc::UnboundedSender<app::UiEvent>)
```

**The loop** mirrors `repl.rs` (whose prompt loop is the reference):
- At the top of each idle pass:
  - `let _ = *finished.borrow_and_update();` (where `finished = driver.subtasks.watch()`);
  - `driver.deliver_subtasks();`
  - send `UiEvent::Status(..)` and `UiEvent::Idle`.
  - `driver.sync_view()`: apply what immediate commands left (ARCHITECTURE.md, C17).
- Then take the next input from the local queue (inputs that arrived during a
  turn), or `select!` on:
  - `rx.recv()`:
    - `Input(text)` runs `driver.input(MessageContent::Text(text), &mut report)` through
      `while_busy` (below);
    - if that returns `Flow::Exit`, send `UiEvent::Exit` and break;
    - `Exit` or a closed channel breaks.
  - `sleep(driver.next_wait())` when it is `Some`: run `driver.run_due(&mut report)` if `driver.task_due()`,
    also through `while_busy`. A due task makes the session busy too: the task sends `UiEvent::Busy`
    first, so the spinner shows, and the UI gets no `Idle` until it ends.
  - `finished.changed()`: continue (the next idle pass hands the subtask back).
  - `view.effect_recorded()`: continue (an immediate command finished after its turn).
- **`while_busy`** pins the turn's future and `select!`s it with `rx.recv()`. An
  immediate command (`commands::immediate`) is answered from `driver.view()`
  on a task of its own, which sends `UiEvent::Reply`; the turn and the
  spinner go on. Any other message goes to the local queue. A closed channel
  queues `Exit` for after the turn.
- After the loop, `driver.shutdown("prompt_input_exit").await`.

**The `report` callback** turns each `TurnResult` into what the UI shows:
- a local command result (`num_turns == 0 && stop_reason.is_none()`) becomes
  `UiEvent::Reply { text: result, is_error }`;
- a blocked prompt (`prompt_blocked`) becomes `Reply { is_error: true }`;
- an interrupted turn becomes `Reply { text: "Interrupted · What should KodAct do instead?" }`;
- a model turn that errored becomes `Reply { text: errors or result, is_error: true }`;
- a successful model turn sends nothing: its text already streamed.

**The status snapshot** after each input (`app::StatusView`):
- model: `driver.handle().model()`;
- mode: `driver.handle().permissions.read().unwrap().mode.as_str()`;
- cwd: `driver.info.cwd`;
- cost: `driver.engine.state.total_cost_usd`;
- context %: `driver.engine.state.context_tokens` over the model's window
  (`kodact_api::models::model_info(model).context_window`).

Also send `UiEvent::Commands` from `driver.catalog.catalog_json(Surface::Tui)`
(fields `name`, `argumentHint`, `description`). Send it once at start and
again whenever the list changes: `/clear`, `/resume`, `/branch`, `/cd` and
the reloads (`/reload-skills`, and `/hooks add` or `/agents create`, which
reload) can change it, and a reload keeps the session id.

**`TuiSink`** (an `EventSink`) sends every `EngineEvent` as
`UiEvent::Engine(e)`. The channel is unbounded, so it never blocks the engine.

**`TuiPrompter`** (a `PermissionPrompter`) creates a `oneshot`, sends
`UiEvent::Ask { prompt, reply }` and awaits the answer. If the receiver is
dropped (UI gone), it answers
`Deny { message: "The UI closed.", interrupt: true }`.

**Building the session** copies `repl::run`:
- `launch_options`;
- `connect_mcp`;
- `build_session(lo, sink, prompter)`;
- `Driver::new(session, Surface::Tui, Some(mcp))`;
- `driver.set_rebuild(..)`;
- take `let live = driver.live();` **before** moving the driver into its task.
  The UI uses `live` for interrupts and modes.

### UI state (`tui/app.rs`, done)

Public surface the loop uses:
- `App::new(theme, history)`;
- `app.on_term_event(crossterm::event::Event) -> Vec<Action>`;
- `app.on_event(UiEvent)`;
- `app.take_pending() -> Vec<Line>`: lines for scrollback, already styled
  but not wrapped;
- `app.next_queued() -> Option<String>`: call after every event; send what it
  returns;
- `app.close_dialog_if_stale()`: call after an interrupt;
- `app.exit` and `app.dirty`;
- read-only state the renderer draws from:
  - `editor`, `live` (the unfinished answer line);
  - `busy`, `busy_since`, `activity`, `queued`;
  - `dialog_text()`, `dialog_options()`, `dialog_selected()`;
  - `menu()`, `menu_selected`;
  - `status`, `hint`.

`Action` and how the loop applies it:

| Action | The loop does |
| --- | --- |
| `Send(text)` | `to_session.send(ToSession::Input(text))` (the app already echoed it and marked itself busy) |
| `Interrupt` | `live.handle().interrupt()`, then `app.close_dialog_if_stale()` |
| `SetMode(m)` | `live.handle().set_permission_mode(m)` |
| `Exit` | `to_session.send(ToSession::Exit)`, then wait (at most 5 s) for the session task to end |

### Renderer (`tui/render.rs`)

```rust
pub struct LiveView { pub lines: Vec<Line<'static>>, pub cursor: Option<(u16, u16)> }
pub fn live_view(app: &App, width: u16, max_height: u16) -> LiveView
```

The live region, top to bottom (each part only when present):
1. `app.live`, the unfinished answer line, rendered with
   `text::Markdown::line` and wrapped. At most 3 rows, the last ones.
2. The spinner while `busy`:
   - braille frames `⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏` (`kodact_core::glyphs::SPINNER`),
     advanced every 80 ms from `busy_since`;
   - then `{activity}… ({secs}s · esc to interrupt)`, in the accent style;
     while a permission, plan or question dialog is open,
     `Waiting for your answer… ({secs}s)` instead.
3. A dialog: a rounded box (`Block::bordered().border_type(Rounded)`) with:
   - the title (`dialog_text().0`) and body lines; a permission dialog for
     Edit, MultiEdit or Write shows the lines it removes (`  - `, red) and
     adds (`  + `, green), at most 8 of each; Bash shows the command and its
     description; the generic reason "this tool requires permission" is left
     out;
   - the numbered options from `dialog_options()`, the selected one marked
     `❯` and styled `theme.selected()`, descriptions dimmed on the right;
   - a footer `Enter to select · Esc to cancel`.
4. Queued messages: `  ⏎ {text}` (dim).
5. The input box:
   - a dim `─` rule above and below;
   - rows prefixed `> ` (first) or two spaces;
   - wrapped by character to `width - 2` so the cursor maths is exact;
   - the cursor goes at the editor position (`editor.position()` mapped
     through the wrap);
   - an empty, idle prompt shows a dim placeholder: `Try "explain this repo" · / for commands`.
6. The `/` menu: up to 8 rows of `/name args` padded to the longest name,
   then the description (dim). The selected row uses `theme.selected()`.
7. The status line:
   - left: the hint if one is fresh (under 2 s), else the mode:
     - `» accept edits on (shift+tab to cycle)`;
     - `‖ plan mode on (shift+tab to cycle)`;
     - `! bypass permissions on`;
     - `? for shortcuts` for the default mode;
   - right: `{model} · {ctx}% context · ${cost:.2}`.

If the lines exceed `max_height` (the terminal height minus 1), drop rows
from part 1, then part 4, never from the dialog or the input.

### Terminal (`tui/mod.rs`)

```rust
pub async fn run(prompt: Option<String>, o: Opts) -> Result<i32, Fail>
```

**Set up:**
- `enable_raw_mode`, `EnableBracketedPaste`;
- if `crossterm::terminal::supports_keyboard_enhancement()`, also
  `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)`, so Shift+Enter
  is a distinct key;
- a panic hook that restores the terminal (pop flags, disable paste, disable
  raw mode, show the cursor) and then calls the previous hook;
- restore the same way on every exit path, through a guard whose `Drop`
  restores.

**`Screen<B: Backend>`** wraps a `Terminal<B>` with `Viewport::Inline(h)`
and a function that makes a fresh backend (so tests run it on a
`TestBackend`):
- `set_height(h)`: when `h` differs, call `terminal.clear()` (the cursor
  goes to the viewport's top and everything below is cleared), drop the
  terminal, and create a new one with `Viewport::Inline(h)`. ratatui 0.29
  can't change an inline viewport's height in place. Grow at once; shrink
  only when idle or when a dialog closes, to avoid flicker while streaming.
- `commit(lines)`: wrap with `text::wrap(lines, width)` and write them with
  `terminal.insert_before(n, |buf| Paragraph::new(lines).render(buf.area, buf))`
  in chunks of at most 100 rows.
- `draw(&LiveView)`: render the lines with a `Paragraph` into the frame and
  set the cursor. A view shorter than the viewport (it hasn't shrunk yet) is
  drawn at its bottom, so the input box doesn't jump.

**The loop**, with frames drawn at most every 16 ms:

```text
loop {
  if !app.pending.is_empty() { screen.commit(app.take_pending()) }
  if app.dirty (or the spinner is due): view = render::live_view(..); screen.set_height(view.lines.len()); screen.draw(&view)
  if app.exit { break }
  while let Some(t) = app.next_queued() { to_session.send(Input(t)) }
  select! {
    ev = events.next()         => for a in app.on_term_event(ev) { apply(a) }
    ev = ui_rx.recv()          => app.on_event(ev)   // None: the session ended, exit
    _  = tick(120ms), if app.busy || a hint is showing => app.dirty = true
  }
}
```

Other events:
- `Event::Resize` marks the frame dirty; `terminal.autoresize` runs inside
  `draw`.
- Ctrl+L: `terminal.clear()` and a full redraw.

**On exit:**
- clear the live region;
- restore the terminal;
- print `Resume this conversation with: kodact --resume <id>`;
- save the history.

**Prompt history** is
`kodact_config::state_dir().join("history.jsonl")`, one JSON line per prompt
with `{"text", "cwd"}`:
- load the last 500 entries for this project directory into
  `App::new(.., history)`;
- append each sent prompt;
- the file is mode 0600.

**Choosing the UI** (`main.rs`):
- open the TUI when stdin and stdout are both terminals and neither
  `--no-tui` nor `KODACT_TUI=0|false|off` is set;
- otherwise use `repl::run`.

`term::Term` has `stdin_tty`. Add a `stdout_tty` field
(`std::io::stdout().is_terminal()`). Add `--no-tui` to `args.rs` and
`docs/CLI.md`, and pass `term::get().color` to `Theme { color }`.

## Rendering rules (implemented in `app.rs` and `text.rs`)

- **Styles** (`look.rs`, `/theme <style>`, the `tuiStyle` setting; the
  first is the default). Each sets the full-screen front page (`front.rs`),
  how the conversation is written (`app.rs`), the approval's frame and the
  input (`render.rs`); colours (`/theme dark|light|none`) apply to all:
  - `frame` (Essentials): only what is used before the first message. The
    mark beside the wordmark (`Kod` in paper, `Act` in lime, over an olive
    shadow); twice the size only on a first run in a folder (no earlier
    conversation there) from 110 columns, normal size after that, so the
    input leads (without colour the letters alone; under 57 columns the
    name in one row). "Code, then act, on the model you choose.", the
    input (at least as wide as the name above it), then, in the panel's
    text column, one `/resume` line for the last conversation, or on a
    first run `Try "explain this project" · /init writes KODACT.md for it`.
    An MCP server that failed to start adds one line (`● figma didn't
    start · /mcp`). The status line shows the folder and branch and the
    keys that help now, on the front page too; the version is in
    `/status`. Steps with a dot; approvals in lime corner marks; what you
    sent is echoed as the panel is drawn, its edge in the mode it was sent
    in.
    The input is a raised panel rather than a box: the text on a row of
    padding, a row of air, then the permission mode, the model, where it
    runs, and the context meter with what the session has spent. The mode is
    never dropped: a narrow panel loses the endpoint, then the context,
    then shortens the model. The
    panel's left edge and the mode's name take the mode's colour (Default
    lime, Plan teal, Auto violet, Accept edits amber, Bypass permissions
    red, Don't ask quiet). The status line under the panel keeps the folder
    and branch on its left (the panel names the mode), and on its right the
    keys that help now: `/`, `@`, `shift+tab` and `?` with nothing typed; `enter` and `shift+enter`
    while typing; `↑↓`, `tab` and `esc` in a menu; `esc` while the model
    works; nothing while a dialog is open (it says its own keys). The `/`
    and `@` menus hang under the panel, on its surface; Ctrl+R's search
    sits in its column. While the model works the empty panel says "Type to
    queue a message; esc stops the turn". In 16 colours the panel has no
    grey (the nearest is too light behind text), only its edge.
  - `glow`: centred, the wordmark lit from the top with sparks and a glowing
    seam, a lime-framed input with the mode and model under it. Your words
    on a tinted band, finished steps with a ✓, answers plain.
  - `card`: a card with the mark beside the wordmark and the session's
    model, tools and folder; turns headed `you · 14:02` and `kodact`.
  - `ledger`: aligned left, the wordmark in lime outline, past work
    numbered beside this folder, the input docked at the bottom under a
    rule. A column of times and voices; steps as plain quiet lines.
  - `status`: the wordmark over boxed readouts of the model, tools and
    folder; in the conversation, a bar of readouts across the top and
    approvals as a centred sheet.
  - `bare`: the name in the corner, one question and the input in the
    middle, recent work under it. Plain steps, a bold `›` for your words,
    the input and approvals under a rule, without boxes.
  A terminal too narrow for a style's front page gets the bare one. The
  names ForgeCLI used (`forgeframe`, `molten`, `anvil`) still choose
  `frame`, `glow` and `card`.
- **Full screen** (the default; `KODACT_TUI=inline` for the inline
  viewport): the UI takes the terminal's alternate screen and keeps the
  transcript itself (`mod.rs` `Full`, `render::full_view`). Before the first
  message, the front screen fills the window: the wordmark and tagline, the
  centred input with the mode, model and endpoint under it, the resume line
  and any notices, centred down the screen, with the status line on the
  last row. After it, the transcript sits above the live region (a short one
  just above it), scrolled with PgUp / PgDn and the mouse wheel (Shift+drag
  selects text while the mouse is captured); sending goes back to the
  newest lines. On exit the transcript is printed into the terminal's
  scrollback, wrapped to its width.
- **The front screen** (`brand::welcome`), printed once at the start,
  centred across the terminal and down the room above the input: the KodAct
  wordmark (`Kod` in paper and `Act` in lime, drawn in half blocks, 52
  columns by 5 rows; `KodAct` in one row on a narrower terminal) and the
  mark (a lime tile with an ink bar and a run triangle: code, then act), the tagline, and `/resume` with the newest other
  conversation here. Until the first message, the input is a centred box at
  most 80 columns wide, with the mode, the model and its endpoint on the
  line under it, and the status line shows the version on its right. The
  status line shows the folder on its left (its end kept, at most half the
  line) until a mode or a hint takes the place.
- **Assistant text** streams into `app.live`. Each complete line moves to
  scrollback. The first line of a text block gets `• `, the others two
  spaces.
- **Markdown**, one line at a time (`text::Markdown::push`), so streamed
  text is drawn as it arrives:
  - headings `#` to `######` (a space after the hashes): level 1 bold in
    lime, level 2 bold in teal, level 3 bold in violet, the rest bold;
  - bullets `-`, `*`, `+` as `•`, `◦`, `▪` by indent level, in lime, teal
    and violet; numbered items (`1.` or `1)`) with the number in teal;
    tasks `- [ ]` as `○` and `- [x]` as a green `✓` with the text struck through;
  - quotes `>` (nested `>>`) as a violet `│` per level, the text dim and
    italic;
    rules (`---`, `***`, `___`) as a hairline;
  - code fences (```` ``` ```` or `~~~`, closed only by the same run) open
    with `╭─ lang`, put each line after a `│ ` gutter and close with `╰─`.
    Lines are highlighted by token (keywords, strings, numbers, comments,
    type names) for Rust, Python, JS/TS, Go, C-family, shell, SQL and data
    formats; a `diff` fence colours `+` and `-` lines; other languages keep
    one quiet code colour;
  - inline: `**bold**`/`__bold__`, `*italic*`/`_italic_` (not inside
    `snake_case`), `~~strike~~`, `` `code` `` (any backtick run), links
    `[text](url)` underlined with the URL after them, `<https://…>` and
    bare URLs underlined, `\` escapes; emphasis nests;
  - table rows are held until the table ends, then drawn with aligned
    columns (`--:` aligns right), `│` between cells, a `─┼─` rule and a
    bold teal header.
- **Text colours**: the model's words in a soft paper (`ANSWER`, 13:1),
  bold in the brightest paper (`STRONG`), inline code in peach on a shade of
  its own (`CODE_BG`); the person's words, typed and echoed, in the brighter
  `PROMPT_FG`, so the two voices stay apart. Quiet text (`SUBTLE`) is at
  least 4.5:1 on the page and on the panel in both themes; the placeholder
  is `MUTED`. In 16 colours the terminal's own text colour is kept.
- **Colours** (`palette.rs`): KodAct's ink, paper and lime, with teal and
  violet beside it. Ink and paper for text and surfaces, lime for the brand
  and for what needs the person; teal and violet for the structure of an
  answer (headings, lists, quotes, table headers, inline code in peach,
  code keywords violet, strings lime, types teal, numbers amber); green, red
  and amber only where they mean done, failed or a warning.
- **Wrapping** (`text::wrap`): a wrapped row continues under the text, past
  the line's indent and its marker (`• `, `› `, `↳ `, `- `, `1. `).
- **Tool calls**: a dot, a verb and what the call works on (`app::tool_header`):
  `Running cargo test` with a turning lime dot while it runs, then
  `● Ran cargo test`, green when it worked and red when it failed. Verbs:
  Ran, Read, Edited, Wrote, Searched, Listed, Fetched, Searched the web,
  Delegated, Updated the plan, Used skill; any other tool is `Called`, with
  an MCP tool named `server.tool`. Their results (`app::result_lines`), under
  a `└`, red on error:
  - Read: `40 lines` or `40 of 120 lines`;
  - Edit, MultiEdit, Write: `1 addition, 1 removal`, then the changed lines
    with their numbers (at most 16 rows): from 140 columns side by side,
    the old line on the left in red and the new one on the right in green;
    narrower, one column, `-` red and `+` green. The diff is laid out each
    time the transcript is (`app::lay_out`), so a resize redraws it; a new
    file: `12 lines, a new file`;
  - Bash: a command with a description reads as its purpose, then the
    command on its own line in the code colour (`✓ Run the cart tests` /
    `$ python3 -m pytest -q`); without one, `Ran <command>`. Then its last 3
    output lines, or `(no output)`;
  - anything else: `first line (+N lines)`.
- **While the model writes a tool call**, the working line says what it is
  preparing: `Preparing an edit to cart.py…`, `Writing notes.md…`,
  `Preparing a command…`; the file name once its path has arrived.
- **Sub-agents**: a running one shows its step count on its row and its
  last 3 steps in a tree under it; once done, the transcript keeps its first
  step, `… N more`, and its last two. The plain styles keep one line.
- **The turn footer**: under each answer, `■ Default · opus · 12s · 1.2k
  tokens`: the mode (square and name in its colour), the model, the time and
  the tokens the turn used. The UI shows no money anywhere; `/cost` and
  `/usage` give it when asked.
- **The sidebar** (full screen from 140 columns, Ctrl+B, the
  `toggleSidebar` action). Its sections keep one order, so the eye learns
  where things are; each heading counts what it holds, flush right:
  - the session's title (its first prompt);
  - **Context**: the room left, the tokens used and the model;
  - **Plan** (`1/4`): the plan from TodoWrite, done steps quiet with `✓`, the
    active one in lime, the rest open `○`;
  - **MCP** (`2/3 up`, or `1 failed` in red): each server and what it
    brings, `● jira  12 tools` in green, a failed one `× figma  failed` in
    red, one still connecting `connecting`, a disabled one `○ … off`;
  - **Skills** (`14 · /skills`): how many there are and the command that
    lists them, then only the skills this session used, the latest first,
    `▸ verify  ×2`; a skill counts as used when the model or a sub-agent calls
    it, or when you type it as `/name`;
  - **Agents** (`2 running`), only while sub-agents run: each with what it
    was asked and its step count, then its kind and how long it has run, and
    its last 3 steps as a tree;
  - the folder and the branch in the session pane under its title.

  Work in flight has a mark that fills: a braille cell that fills from the
  bottom, dot by dot (`⡀ ⣀ ⣄ ⣤ ⣦ ⣶ ⣷ ⣿`), and drains again, its colour
  breathing with it (dim nearly empty, full when full, in 24-bit colour),
  each at its own pace: the active plan step during a turn (half full `⣤`
  while the session waits), a server connecting, a sub-agent working. The
  context gauge uses the same cell, filled to how much is used, and the
  sidebar adds a bar under it that fills left to right, dot by dot. A finished state
  is still: `●` connected, `✓` done. Every state has a word or a shape as
  well as a colour. Each section is a pane: a rounded hairline box on the
  window's own background, its name and count set into the top border
  (`╭─ Plan ──── 1/4 ─╮`), the session's title in a pane named `session`; a
  pane with nothing under its heading says `none used yet`. The sidebar is a
  quarter of the window, 34 to 46 columns, and its panes are drawn at that
  width each frame, so resizing redraws them whole. A pane shows whole or not
  at all: short of room, the blank rows between panes go first. The sidebar fills the window: with room to spare every
  section shows in full; short of room the lowest sections tighten first
  (a sub-agent keeps only its latest step, the skills their five latest, a
  long plan step two rows), then give way, lowest first: agents, skills,
  MCP, context. The plan never gives way. Never on the welcome page,
  inline, or on a narrower window.
- **Approvals**: a box titled `◆ Approve <tool>?`. A command shows its lines
  and description, an edit its change, and any other tool every argument it
  will be sent (each line of each value). Choices: `Allow once` (y), the
  suggested `Always allow …` (a), `Deny, and tell KodAct what to do` (n);
  also ↑↓ and Enter, or 1 to 3. While it is open the working line says
  `◆ Waiting for your answer (Ns …)`, timed from when the question was
  asked, not from the turn's start.
- **The input** sits in a rounded box; the status line under it shows the
  model, the context meter and the tokens used (no money). On a first run in
  a folder the default style's placeholder suggests a request: `What shall
  we build? Try "fix the failing test"`.
- Events of sub-agents (those with a `parent_tool_use_id`) are not shown.
- **Your prompts**: `> text`, in the user style.
- **Local command output**: `  ↳  ` on the first line, then indented.
- **Glyphs** live in `kodact_core::glyphs` (answer `•`, tool `›`, result `↳`,
  the spinner, the mode marks), so `/export` and the line REPL use the same
  marks.
- **Notices**: dim, yellow (warning) or red (error).
- **System events**: `compact_boundary` shows "Conversation compacted.", and
  `model_fallback` a warning.
- **Colour**: `Theme { color: false }` (`NO_COLOR`, `--color never`) uses
  only bold, dim and reverse.

## Keys (implemented in `app.rs`)

| Key | Does |
| --- | --- |
| Enter | Send. While a turn runs, a message is queued, but an immediate command (`/status`, `/usage`, `/tasks`, `/context`, `/mcp`, `/btw`, `/keybindings`, `/terminal-setup`) is answered at once. In the `/` menu, run the command, or complete it when it takes arguments |
| Shift+Enter, Alt+Enter, Ctrl+J, `\` then Enter | New line |
| Esc | Interrupt the turn; close the menu; cancel a dialog (its last option); twice on an empty prompt: `/rewind` |
| Ctrl+C | Clear the input; interrupt the turn (and deny an open dialog); twice on an empty prompt: exit |
| Ctrl+D | Exit (empty prompt) |
| Shift+Tab | Next permission mode: default, acceptEdits, plan |
| Up / Down | Line up/down; history on the first/last line; menu or dialog selection |
| Tab | Complete the highlighted `/` command or `@` path |
| Ctrl+R | Search prompt history |
| Ctrl+A / Ctrl+E, Home / End | Start / end of line |
| Ctrl+W, Alt+Backspace | Delete the word before the cursor |
| Ctrl+U / Ctrl+K | Delete to the start / end of the line |
| Alt+B / Alt+F, Ctrl+Left / Ctrl+Right | Word left / right |
| 1-9 in a dialog | Choose that option |
| Space in a multi-select question | Toggle the option |
| Ctrl+L | Redraw |
| Ctrl+B | Show or hide the sidebar (full screen, 140+ columns) |

Pasted text (bracketed paste) is inserted as typed, newlines included.

**Rebinding** (`tui/keys.rs`): `keybindings.json` (CLI.md, "Key bindings")
is read once at start into a `Keymap`. Every key event goes through
`Keymap::translate` before `App::on_key` (and before the loop's Ctrl+L
check): a bound key becomes its action's default key, an unbound one is
dropped, anything else passes. So `on_key` keeps one set of keys, and the
bindings work in dialogs and screens too. `keys_text` shows each action's
keys in effect, the file and the unbound keys.

## Dialogs (implemented in `app.rs`)

| Dialog | Options | Answer |
| --- | --- | --- |
| Permission | 1. Yes | `Allow` |
| | 2. Yes, and don't ask again for `<rule>`, or "Yes, and allow all edits this session (shift+tab)" for an edit (only when the prompt has suggestions) | `Allow` with the suggestions as `updated_permissions` |
| | 3. No, and tell KodAct what to do instead (Esc) | `Deny { interrupt: true }` |
| AskUserQuestion | The options, then "Type an answer"; multi-select toggles with Space | `Allow` with `input.answers = {question: answer}` (the shape `LinePrompter` uses) |
| Plan approval (the plan goes to scrollback first) | 1. Yes, and accept edits without asking | `Allow` plus `setMode acceptEdits` (session) |
| | 2. Yes, and ask before each edit | `Allow` |
| | 3. No, keep planning (Esc) | `Deny { interrupt: true }` |

## Screens

A screen is what a command typed without arguments shows instead of its
text answer: `/diff`, `/context`, `/hooks`, `/agents`. It stays an inline
viewport: a screen is a dialog in the live region, at most the terminal
height minus the status line, and long content scrolls inside it.

- **Data** comes from kodact-core (`commands::screens`), as pickers do:
  a `Screen` is a title and rows of tagged text (`Tone`: plain, dim, bold,
  accent, added, removed, or a context part). A row may have an action:
  - `Jump(row)`: Enter moves there (a file in `/diff`); Esc comes back;
  - `Run(text)`: Enter runs the command text, as if typed;
  - `Confirm { question, command }`: a Yes/No picker first;
  - `Form(form)`: opens a form.
- **Forms** are fields (free text, one-of choice, any-of multi-select) and a
  command template; `{0}`, `{1}`, ... are replaced by the values,
  shell-quoted. Tab and Up/Down move between fields, Left/Right choose,
  Space toggles, Enter runs, Esc cancels. The form shows the command it will
  run.
- **Everything a screen changes is command text** the person could type
  (`/hooks remove PreToolUse 1`), so the text commands stay the answer on
  every other surface (`-p`, stream-json, the line REPL) and their tests
  don't change.
- **The session task** answers a bare screen command with
  `UiEvent::Screen(..)`, after pickers. `/context` is immediate, so it opens
  mid-turn too, built from the `SessionView`.
- **Questions win:** a screen or picker that arrives while a permission
  prompt or question waits for its answer (a `/context` typed mid-turn) is
  held and shows once the question is answered. Replacing that dialog would
  drop its reply, which the engine reads as "the UI closed".
- **Keys:** Up/Down move the highlighted row; PageUp/PageDown (and Space) move
  by the rows shown; Home/End; Enter (or Right) acts on the row; Esc (or
  Left) goes back from a jump, else closes; `q` closes.
- **Drawing** (`render::viewer`, `render::form`): rows are cut to the width,
  never wrapped, so scrolling counts rows exactly. The footer shows the rows
  shown ("12-30 of 200") and the keys. The highlighted row keeps its colours
  and turns bold, with `❯` in front. Without colour, diffs keep their `+`
  and `-` markers and the context grid uses a letter per part.

The screens:
- **`/diff`:** git's uncommitted changes (`git diff HEAD`), or outside git
  the files KodAct changed against how they were. First the files, each with
  `+added -removed` (Enter jumps to its hunks), untracked files and the files
  each prompt changed; then each file's hunks: additions green, deletions
  red, hunk headers dim. Built from the same data as the text `/diff`.
- **`/context`:** a 10×10 grid, each cell 1% of the window, coloured by the
  part that fills it (system prompt, built-in tools, MCP tools, skills,
  memory, messages, free; a used part shows at least one cell), then the
  legend with the numbers of the text `/context` (same `ContextData`), the
  auto-compact point and suggestions. `/context all` stays text.
- **`/hooks`:** "+ Add hook…" (a form: event, matcher, command, scope, which
  runs `/hooks add ...`), then every event with its hooks, numbered as
  `/hooks` numbers them, each with its source and timeout. Enter on a user,
  project or local hook asks, then runs `/hooks remove <Event> <n>`. The
  session reloads after a change, so the hook applies at once.
- **`/agents`:** "+ Create an agent…" (a form: name, description,
  instructions, tools as a multi-select of the session's tools, model,
  scope, which runs `/agents create ...`), then each agent with its source,
  model and tools.

## Testing

- **Unit tests, done:** `tui::editor`, `tui::text` and `tui::app`.
  Run them with `cargo test -p kodact-cli --bin kodact tui`.
- **Renderer (done, `tui::render::tests`):** `ratatui::backend::TestBackend` snapshot tests of
  `live_view` drawn into a fixed area:
  - idle;
  - busy with a spinner and queued input;
  - the `/` menu;
  - each dialog;
  - a narrow width (20 columns);
  - `Theme { color: false }`.
  Compare `buffer` text rows; colours are checked through a few cells.
- **Session (done, `tui::session::tests`):** build a session with `MockProvider` (as
  `crates/kodact-core/src/driver_tests.rs` does in `driver_with`), run
  `session::run` on a task, and drive it through the channels:
  - a text turn ends with `Idle`, and `Engine` events carry the text;
  - a `/status` reply arrives as `Reply`;
  - a permission prompt arrives as `Ask`, and the oneshot answer reaches the
    tool;
  - a queued input is sent after `Idle`;
  - `/exit` gives `Exit`.
- **Terminal (done, `tui::tests`):** keep `mod.rs` thin. Test `Screen::commit`'s chunking
  and wrapping through a `TestBackend` with `Viewport::Inline`; `insert_before`
  works on it. Check by hand in a real terminal (the checklist below).

## Phases

1. **The session view** (finish it):
   - `session.rs`, `render.rs` and `mod.rs` as above;
   - wiring into `main` with `--no-tui` and `KODACT_TUI`;
   - persisted history;
   - tests;
   - docs:
     - `docs/CLI.md`: the interactive section and `--no-tui`;
     - `docs/PARITY.md`: TUI rows;
     - `CHANGELOG.md`;
     - `docs/CHECKLIST.md`: the manual steps below.
2. **Pickers** (done). A `Picker` dialog (title, rows, filter by typing).
   The rows come from `kodact_core::commands::picker::picker(driver, text)`,
   so they number and name things exactly as the commands do. Each row is a
   `Pick`: `Run(text)`, `RunThenEdit` (run, then put the rewound prompt back
   in the input box), `Step(text)` (a second picker) or `Edit(text)` (put
   a command in the input box to finish by hand).
   - The session task answers an input that is one of these commands
     without its argument with `UiEvent::Picker(..)` instead of running it,
     so aliases (`/undo`, `/allowed-tools`) open pickers too.
   - A `Step` is `ToSession::Picker(text)`, answered the same way.

   The pickers:
   - `/model` with no argument: `driver.model_choices()`, the models the
     endpoint lists (`Provider::list_models`, asked once per session) with
     the current one first, plus "Another model…" (puts `/model ` in the
     input box). KodAct shows no built-in catalogue;
   - `/resume`: `kodact_session::SessionStore::list(cwd)`;
   - `/rewind`: `driver.engine.prompt_points()`, then a second step for the
     action (both, conversation, code, summarize from, summarize to);
   - `/output-style`: `driver.catalog.styles`;
   - `/permissions`: each rule (choosing it removes it), "Add an allow/ask/deny
     rule…" (puts `/permissions add <behavior> ` in the input box), and the
     full list.

   Choosing a row sends the existing argument form (`/model <id>`,
   `/resume <id>`, `/rewind 2 code`, ...), so the commands don't change. Also in
   this phase:
   - Ctrl+R reverse history search: typing searches back through prompts
     (newest first), Ctrl+R again goes older, Enter or any editing key keeps
     the match in the input box, Esc or Ctrl+G goes back to what was typed;
   - `@` file completion: a menu of paths from the `ignore` crate's walk of
     the project (`.gitignore` respected, at most 20,000 paths, walked again
     after `/cd`), file-name matches first. Tab or Enter completes, quoting a
     path with spaces (`@"my notes.md"`). Sending attaches the file (the
     driver does it for every surface; docs/CLI.md, "`@` mentions"), and the
     scrollback shows a dim `(attached: a.rs, b.rs)` under the prompt.
3. **UI-only commands** (done). `/theme` and `/statusline` are kodact-core
   commands (they save settings through the driver); the session task sends
   `UiEvent::Theme` when the `theme` setting changes and runs the
   `statusLine` command after each input (`UiEvent::StatusLine`). `/copy`,
   `/keybindings` and `/terminal-setup` need the terminal, so the session task
   answers them before the driver sees them; the driver answers "works only
   in the terminal UI" on other surfaces. `?` on an empty prompt shows the
   key table too.
   - `/theme`: dark, light, no colour; saved as `theme` in user settings.
   - `/copy [N]`: the Nth latest answer to the clipboard via OSC 52.
   - `/keybindings`: shows the key table.
   - `/statusline`: runs the `statusLine.command` setting with the session
     as JSON on stdin (model, cwd, session id, cost, context) and shows its
     first output line as the status line.
   - `/terminal-setup`: explains Shift+Enter for terminals without the
     keyboard protocol.

   Register them in `BUILTINS` with `Surfaces` set to the TUI only, so
   `/help` in other modes leaves them out.
4. **Screens and settings** (done). The pattern is in "Screens" above:
   kodact-core builds a `Screen` (and any `Form`), the TUI draws it in the
   live region, and every change is command text.
   - A scrolling viewer dialog (`DialogKind::Viewer`) and a form dialog
     (`DialogKind::Form`), with `TestBackend` tests at 40 and 100 columns,
     colour and no colour.
   - `/diff` viewer, `/context` grid, `/hooks` editor
     (`/hooks add|remove` argument forms), `/agents` wizard
     (`/agents create`).
   - Key rebinding from `keybindings.json` ("Keys", "Rebinding").
   - `/color <name>` (the accent colour for this session) and `/focus` (tool
     calls stay out of the scrollback), answered by the app itself like
     `/keybindings`. `/tui` and `/scroll-speed` are out: KodAct has one UI,
     and the terminal scrolls its own scrollback.

## Manual checks

The terminal itself (raw mode, the keyboard protocol, scrollback in a real
emulator) is checked by hand: `docs/CHECKLIST.md`, "Terminal UI".
