# Plan: Windows desktop, files you watch being made, permissions that keep their word

Written 2026-11-24 from a real session on Windows ("make a PowerPoint about
cancer for kids", KodAct desktop, deep-thinking gateway, Bypass permissions).

**Scope: the desktop app ships for Windows only, so every piece of this plan
is built and tested for Windows 10/11 only.** Linux and macOS paths in the
desktop (`/Applications/LibreOffice.app`, WSL-only branches, poppler) are
dropped from the file-generation path. The KodAct CLI stays cross-platform,
because the evals (Harbor) run it on Linux. CLI changes here must also work
on Windows.

## Status (2026-11-24)

Built and pushed on `claude/funny-turing-iu00qw` in both repositories. The
app test `scripts/e2e/office_live.sh` (KodAct-Desktop) passes on Linux with
LibreOffice; the PowerPoint/Word/Excel COM route is written and unit-tested,
and still needs a run on a Windows machine with Office.

| Step | Status |
|---|---|
| A1 report the real mode | done (CLI) |
| A2 always pass the switch flag, keep the allowance, say why a switch failed | done (desktop) |
| A3 plugin and skill folders readable | done (CLI) |
| A4 Auto knows Windows removals; risky words only as words (`add` ≠ `dd`) | done (CLI) |
| B5 `init`/`add` for decks, Word and Excel, themes from the audience | done (skill) |
| B6–7 live card "Writing…", panel opens and redraws, Markdown too | done (desktop) |
| C8–9 Office COM renderer first, LibreOffice next; `KODACT_OFFICE_RENDER` | done; Windows run owed |
| D10 recovered failures fold; pages stay up while the next version draws | done (desktop) |
| D11 PowerShell colours (own grammar) | done (desktop) |
| D12 deck layouts: charts, tables, timeline, compare, grid, kpis, cycle, pyramid… | done (skill) |
| Markdown callouts (`> [!NOTE]`) | done (desktop) |
| Download: toast with Open and Show in folder | done (desktop) |
| App test | `scripts/e2e/office_live.sh`: passes |
| Eval | `evals/tasks/deck-in-auto`; the `kodact-eval compare` needs a key: owed |

## 1. What went wrong (checked against the code)

| # | What you saw | Cause | Repo |
|---|---|---|---|
| P1 | Bypass on, and "Allow KodAct to run this command?" still came up | `bypass_allowed` isn't saved, and `modes::cli_args` passes `--allow-dangerously-skip-permissions` only if Bypass was allowed **when the chat started**. Switching later is refused by the CLI, the desktop ignores the error (`let _ = live.session.set_mode`), and the chat keeps running in Auto. | Desktop (patch ready) + CLI (**done**) |
| P2 | Auto asked just to read the skill's own scripts | The plugin folder (`%APPDATA%\…\plugin\…`) is outside the chat folder, and read-only commands on outside paths ask | CLI |
| P3 | Auto's rules are written for Unix | Auto stops `rm`, but on Windows a removal is also `del`, `rd /s`, `Remove-Item -Recurse`. Paths come as `C:/…`, `C:\…` and `/c/…`, and case doesn't matter on Windows. | CLI |
| U1 | No file until the reply ended; the task started with a `.md` outline | The card is made from the finished reply. The office skill's only build path is outline `.md` → `create` → done. | Desktop + plugin |
| U2 | The .pptx panel showed paragraphs, not slides | Slides are drawn only through LibreOffice; without it you get the text editor. PowerPoint was installed and unused. | Desktop |
| U3 | `render.ps1` had no colours | No PowerShell grammar; `language_of` has no `ps1` | Desktop |
| U4 | Noisy chat: `which` dumped the whole `PATH`, there were three failed tool checks, and full-size PNGs | The skill makes the model look for tools; long and failed steps aren't folded | Desktop + plugin |
| U5 | Plain default deck; the kids' design took ~40 min of hand-written shape maths | `pptx_tool.py` has one theme and no layouts | Plugin |

## 2. The experience to match (Claude Desktop, as it behaves)

- The **file exists from the first step**, as a card and an open preview,
  and every new version replaces the old one while you watch.
- **Nothing to approve**, because the sandbox draws the line. The tools are
  always there, so the model never checks for them.
- The **skill carries a design brief and a render/check loop** that always
  works.
- **Steps stay small:** one-line summaries, output folded, thumbnails not
  full images.
- **The result shows as it looks:** slide images with Download and "Open
  in PowerPoint". No text editor by default.

## 3. Windows building blocks

| Need | Windows way | Fallback |
|---|---|---|
| .pptx → slide images | PowerPoint COM, from a small PowerShell helper the desktop ships: `Presentations.Open(path, ReadOnly=-1, Untitled=0, WithWindow=0)`, then `Slides(i).Export(png, "PNG", 1600, 900)` | The LibreOffice copy the installer already stages (`installer/stage_libreoffice.sh`, beside `kodact.exe`) → PDF → images |
| .docx / .xlsx → page images | Word `ExportAsFixedFormat` / Excel `ExportAsFixedFormat` to PDF, then the PDF row below | The staged LibreOffice → PDF |
| PDF → page images | `Windows.Data.Pdf.PdfDocument` (WinRT, in Windows 10+), from Rust through the `windows` crate, MIT/Apache | `pypdfium2` in KodAct's Python |
| Seeing file changes | `ReadDirectoryChangesW` (the `notify` crate), 150 ms debounce | Polling the modified time every 500 ms |
| A file still being written | Retry on `ERROR_SHARING_VIOLATION`; copy to `%TEMP%\kodact-render\<hash>` before rendering; skip `~$*.pptx` lock files | — |
| Highlighting | `tree-sitter-powershell` (MIT) added to the editor's grammars | — |

Office COM runs one file at a time on its own thread (STA), with a 60 s time
limit, and is killed if it hangs. Office is never left open with a window.

## 4. The plan, in order

### Phase A: permissions (small, first)

1. **CLI: report the real mode.** **Done**: `system/status` with
   `permissionMode` after every `set_permission_mode`. Test:
   `crates/kodact-cli/tests/modes.rs::a_mode_switch_reports_the_mode_the_engine_is_in`.
2. **Desktop: always pass the switch flag.** Patch:
   [`desktop-bypass-flag.patch`](desktop-bypass-flag.patch). Also save
   `bypass_allowed` in the settings file, and show a toast (and switch the
   menu back) when a mode switch is refused.
3. **CLI: plugin and skill folders are readable.** Call `add_read_dir` for
   each loaded plugin root in `build_session`. Credential files stay guarded
   by the threat scanner.
4. **CLI: Auto knows Windows.** Treat `del`, `erase`, `rd /s`,
   `rmdir /s`, `Remove-Item` (`-Recurse`/`-Force`), `ri` and `Clear-Content`
   as removals. Read `C:\`, `C:/` and `/c/` as the same path, and compare
   paths without case on Windows.
   Unit tests in `kodact-permissions`; they pass on Linux too.

The goal for this phase: in Auto, building a deck in a chat's own folder asks
**nothing** except deleting something outside the folder. In Bypass, it asks
nothing at all.

### Phase B: the real file first, live in front of you

5. **Plugin: build into the real file, not a `.md`.** Add steps to
   `pptx_tool.py`:
   - `init deck.pptx --theme kids` writes a valid 16:9 deck with its title
     slide.
   - `add deck.pptx --layout cards|steps|statement|faces|title --json slide.json`
     appends one slide.
   - `set` / `restyle` edit a slide that is already there.

   Each step saves the file, so the deck grows slide by slide. The skill's
   first instruction becomes "`init` the requested file type before anything
   else". `docx_tool.py init` and `xlsx_tool.py init` work the same way. The
   `.md` outline stays as an input, never as the first thing the person sees.
6. **Desktop: the card appears at the first step.** When a `tool_use`
   (Bash, Write, Edit) names a `.pptx`, `.docx`, `.xlsx` or `.pdf` path,
   put a card in the chat at once with status **Writing…**, and open the
   side panel on it.
7. **Desktop: live redraw.** Watch the file. On each settled change, render
   it again with the section 3 pipeline and swap the images in place: same
   scroll position, the changed slide marked for a moment. The status goes
   Writing → Checking (while `check_tool.py validate`/`render` runs) → Ready,
   or Error with the reason.

### Phase C: a solid preview

8. **Slides as slides.** A .pptx opens as large slide images with a
   filmstrip, page numbers, Download and "Open in PowerPoint". Editing the
   text moves behind an **Edit text** button. Word and PDF files show as
   pages, and Excel as its sheet grid with a "Print view" toggle.
9. **One renderer for the model too.** The desktop sets
   `KODACT_OFFICE_RENDER=powerpoint|libreoffice|none` and
   `KODACT_RENDER_HELPER=<path to the .ps1 helper>`. `check_tool.py render`
   uses the same helper, so the model never checks for tools or writes its
   own `render.ps1`. The skill says: "render with `check_tool.py render`; it
   works or says why."

### Phase D: a quieter chat and polish

10. **Fold noise.** Tool output over 5 lines is folded. A failed step
    followed by its successful retry shows as one row. Image reads show as
    thumbnails. A step's long `PATH` or environment dump is cut to its first
    line plus "…".
11. **PowerShell colours** for `.ps1`, `.psm1` and `.psd1` in the editor and
    in the chat's code blocks, and the same colours for the command line on
    `Bash`/`PowerShell` step cards.
12. **Deck themes.** `kids`, `clean` and `bold`, each a palette, fonts that
    ship with Windows (Century Gothic, Trebuchet MS, Segoe UI) and the shape
    vocabulary: cards, number badges, faces with moods, sun/star/heart/cloud.
    Arc angles are done once inside the tool, never by the model.

## 5. The app test (Windows)

**Where:** `tests/e2e_office_live.rs` in KodAct-Desktop. It runs on a
Windows runner (GitHub `windows-latest`, which has no Office) and on a dev
machine with Office. Cases that need PowerPoint are skipped, with a message,
when `PowerPoint.Application` can't be created.

**How:** a real `kodact.exe` driven by the mock model
(`kodact-test-host::MockApi`), so it needs no key and gives the same result
every time. The mock model:

1. loads the office-documents skill;
2. runs `pptx_tool.py init deck.pptx --theme kids`;
3. runs `pptx_tool.py add …` three times;
4. runs `check_tool.py validate` and `render`;
5. runs `Remove-Item -Recurse pages; pptx_tool.py add …`;
6. replies.

| Check | Covers |
|---|---|
| Launched in Auto, Bypass allowed mid-chat, then step 5: no `can_use_tool` reaches the UI, and the menu agrees with the CLI's `system/status` after every switch | P1 |
| In Auto, steps 1–4 ask nothing. Step 5 asks once in Auto (`Remove-Item -Recurse`) and never in Bypass. | P1–P3 |
| The `deck.pptx` card exists right after step 2's `tool_use`, before the turn ends, and its status passes Writing → Checking → Ready | U1 |
| The panel's render key changes after each `add` (4 different renders), and the slide count shown goes 1 → 4 | U1 |
| With PowerPoint: slide images, not the text editor. Without PowerPoint and LibreOffice: a clear "can't draw slides" note and the Edit text view. | U2 |
| `language_of("render.ps1") == "powershell"`, and a sample script gets more than one highlight kind | U3 |
| A tool result over 5 lines is folded by default, and a fail + retry pair shows as one row | U4 |
| No step in the turn ran `which`, `where`, `Get-Command` or a check for a tool | U4 |

Plus screenshots of the panel after steps 2, 3 and 6, in light and dark, for
the visual check (`/visual-critique:critique-screen`).

**The CLI side:** eval task `evals/tasks/office-deck-no-prompts/` counts
permission prompts and failed tool checks in a deck-building run in Auto.
The `kodact-eval` before/after comparison needs an API key and is still
owed.

## 6. Done means

- In Bypass: no prompts, ever. In Auto: no prompts for work inside the chat's
  folder except deletions outside it.
- The requested file type's card appears within one step of the request and
  updates live, and no `.md` is shown first.
- A .pptx always shows as slides on a Windows machine with PowerPoint or the
  bundled LibreOffice.
- `.ps1` files are coloured.
- The app test passes on `windows-latest`, and the PowerPoint cases pass on a
  machine with Office.

## 7. What it needs

Phase A, steps 3–4 are in this repository and can start now. Everything
marked Desktop or Plugin is in **KodAct-Desktop**, which this cloud session
can only read: it needs write access to that repo. The PowerPoint parts can
be written here, but can only be checked on your Windows machine.
