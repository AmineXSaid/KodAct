# Desktop: permissions that keep their word, and file generation that feels live

Found on 2026-11-24 from a real session ("make a PowerPoint about cancer for
kids", KodAct desktop on Windows, deep-thinking gateway). Most of the work is
in **KodAct-Desktop**. One part is in this repository and is already done.

## What the person saw

| # | Symptom | Cause (checked in the code) | Where |
|---|---|---|---|
| P1 | "Bypass permissions" on, and KodAct still asked "Allow KodAct to run this command?" (`… && rm -rf pages && …`) | The desktop keeps `bypass_allowed` in memory only (`state.rs`, starts `false`), and `modes::cli_args` passes `--allow-dangerously-skip-permissions` only when it is true **at launch**. So a chat started before you allowed Bypass got a CLI that refuses the `set_permission_mode` switch. `Session::set_mode` throws the error away (`let _ =`), the menu says Bypass, and the CLI stays in Auto, where `rm` asks. | Desktop `src/modes.rs`, `src/state.rs::set_mode` |
| P2 | Even in Auto, reading the skill's own scripts asked (`ls <plugin>/skills/office-documents/scripts`) | The plugin folder is outside the chat's folder. Read-only commands on paths outside the working directories ask in every mode except Bypass, while Auto lets the same path through when it is *run* (`python <plugin>/…/pptx_tool.py`). | CLI `kodact-permissions::Engine::decide` |
| U1 | No file card until the reply ended, so there was nothing to click and watch while the deck was built | The card is made from the finished reply's files. Claude Desktop shows the file as soon as the first write starts and updates it in place. | Desktop `ui/file_card.rs`, `state.rs` |
| U2 | The .pptx panel showed numbered paragraphs ("4 / Sometimes a Cell Forgets the Rules / 1 / A cell can…"), not slides | Slides are drawn only through LibreOffice (`office_render.rs`). Without it, `doc_pane.rs` falls back to the text editor. PowerPoint was installed and could have drawn them; the model found that out on its own with a PowerShell COM script. | Desktop `office_render.rs`, `ui/doc_pane.rs` |
| U3 | `render.ps1` showed as plain text | gpui-component's `tree-sitter-languages` has no PowerShell grammar, and `text_editor::language_of` has no `ps1` entry | Desktop `ui/text_editor.rs` |
| U4 | Large tool output in the chat: full Windows `PATH` dumps from `which`, three failed checks for missing tools, every slide PNG shown inline | The skill sends the model to look for tools (`pdftoppm`, `soffice`, `pywin32`) instead of KodAct telling it what the computer has. Each check is a visible step in the chat, and it fails. | Plugin `office-documents/SKILL.md`, `check_tool.py` |
| U5 | The default deck was plain and black on white. The kid design was hand-built, with ~40 minutes of arc-angle maths. | `pptx_tool.py create` has one theme and no shape vocabulary (characters, icons, cards), so every designed deck is written from scratch | Plugin `pptx_tool.py` |

## How Claude Desktop does it (the behaviour to match)

This is described from how the product behaves, not from its source.

- **A sandbox draws the line, not prompts.** File creation runs in a
  container with the tools already installed (python-pptx, LibreOffice,
  poppler, Node). The model never checks whether a tool is there, and there
  is nothing to approve.
- **Skills ship the whole pipeline.** The pptx skill gives a design brief
  (palettes, layouts, "no plain bullets"), a build path, and a thumbnail grid
  script for checking the result. Rendering always works because the tools
  are always there.
- **The file appears early.** The file shows as a card or preview pane during
  the turn and is replaced as new versions are written. You watch it change.
- **Steps stay small.** Tool calls fold into one quiet "working" timeline
  with one-line summaries, and output stays folded unless you open it.
- **The result is shown as it looks.** Slides appear as slide images or
  thumbnails, with Download and "Open in PowerPoint". A text view isn't
  offered for a deck.

## The plan

### Done in this repository (commit on `claude/funny-turing-iu00qw`)

- **P1, CLI half.** After every `set_permission_mode`, granted or refused,
  KodAct CLI sends `system/status` with the mode the engine is really in.
  The desktop already turns that into `ModeChanged`, so with this alone the
  menu goes back to Auto instead of claiming Bypass. Test:
  `crates/kodact-cli/tests/modes.rs::a_mode_switch_reports_the_mode_the_engine_is_in`.

### Desktop, in order

1. **P1, desktop half (patch ready: [`desktop-bypass-flag.patch`](desktop-bypass-flag.patch)).**
   Always pass `--allow-dangerously-skip-permissions`. The flag only lets
   the CLI switch modes; the desktop's Bypass warning stays the gate. Also
   keep `bypass_allowed` in the settings file, and show a toast when a
   `set_permission_mode` reply is an error.
2. **U1, a live file card.** When a `tool_use` for Write, Bash or Edit names
   an output file with `.pptx`, `.docx`, `.xlsx` or `.pdf` (or the
   office-documents skill starts), put a card in the chat right away marked
   "Writing…". Open the panel on it, and redraw it each time the file
   changes on disk (the panel already keys renders by the file's bytes). Its
   status goes Writing → Checking → Ready.
3. **U2, slides everywhere.** Add a second renderer to `office_render.rs`:
   on Windows with PowerPoint, export through COM, the same steps as the
   model's `render.ps1` (`Presentations.Open(…, ReadOnly, Untitled,
   WithWindow=false)`, then `Slide.Export(png, 1600, 900)`). Make the order
   LibreOffice, then PowerPoint, then the text fallback. A .pptx is shown as
   slide pictures with a filmstrip. Editing the text stays possible, but
   behind an "Edit text" button, not as the default view.
4. **U4, tell the model what the computer has.** The desktop already finds
   `soffice` and `KODACT_PYTHON`. Also give the CLI
   `KODACT_OFFICE_RENDER=libreoffice|powerpoint|none` and make
   `check_tool.py render` use it, including the PowerPoint path. The skill
   then says "render with `check_tool.py render`; it always works or says
   why", and the model checks for nothing. Fold failed-and-retried steps
   and long outputs (over 5 lines), and show image reads as thumbnails, not
   full size.
5. **U3, PowerShell colours.** Add a PowerShell grammar
   (`tree-sitter-powershell`, MIT) to the editor's languages and map
   `ps1`, `psm1` and `psd1` to it in `language_of`. Check the licence
   (CLAUDE.md rule 1).
6. **U5, a design layer for decks.** Give `pptx_tool.py create` themes
   (`--theme kids|clean|bold`) and an outline syntax for layouts (cards,
   steps, big statement, faces/icons row), so "make it for kids" is one
   command and a check, not a hand-written renderer.

### CLI, in this repository

7. **P2.** Treat the folders of loaded plugins and skills as read
   directories (`Engine::add_read_dir` in `build_session`). Reading a
   skill's own files then never asks, in any mode. Credential files stay
   guarded by the threat scanner.

## The app test (one scenario covering all of the above)

A desktop end-to-end scenario, `tests/e2e_office_live.rs` in KodAct-Desktop,
run with the mock model (`kodact-test-host::MockApi`) so it needs no key and
gives the same result every time:

Script: the mock model (1) loads the office-documents skill, (2) Writes
`build_deck.py`, (3) runs `python build_deck.py` (makes `deck.pptx`),
(4) runs `check_tool.py render deck.pptx pages/`, (5) runs
`rm -rf pages && python build_deck.py`, (6) replies.

Checks, each tied to a symptom:

| Check | Symptom |
|---|---|
| Launched in Auto, Bypass allowed mid-chat, then step 5: **no** `can_use_tool` reaches the UI, and the mode menu and the CLI's `system/status` agree on every switch | P1 |
| In Auto, steps 1–4 ask nothing. Step 5 asks once (a removal), and in Bypass it asks nothing | P1, P2 |
| A file card for `deck.pptx` exists **after step 3's `tool_use`**, before the turn ends, and its panel redraws when the file changes (two different render keys) | U1 |
| With LibreOffice hidden (`KODACT_SOFFICE` set to a missing path), the panel for a .pptx shows slide images on Windows and says "Install LibreOffice…" only when PowerPoint is also missing | U2 |
| `language_of("render.ps1") == "powershell"`, and the editor puts more than one highlight kind on a sample script | U3 |
| A tool result over 5 lines is folded by default, and a failed step followed by a successful retry shows as one row | U4 |

Run it on Windows (the release target) and Linux, and take light and dark
screenshots of the panel at steps 3 and 6 for the visual check.

For the CLI half, add an eval task (`evals/tasks/office-deck-no-prompts/`)
that counts permission prompts and failed tool checks in a deck-building run
in Auto. The before/after `kodact-eval compare` needs a key and is still
owed.
