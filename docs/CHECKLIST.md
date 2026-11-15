# Manual checks against the real API

The test suite runs against a mock API (`MockProvider`, `MockApi`) and, for
the terminal UI, a pseudo-terminal. It can't show how a real model uses
KodAct's tools and prompts, or how a real terminal emulator behaves. Run this
list with a real key before a release. It is one ordered run in three parts:

- **A. Print mode** (a key; any shell): 12 checks, about 15 minutes.
- **B. Interactive** (a key and a real terminal emulator): 34 checks, about
  60 minutes.
- **C. Special setups** (`NO_COLOR`, a 40-column window, a keyboard-protocol
  terminal, tmux, SSH, bubblewrap, another agent's config, the line REPL):
  9 checks, about 25 minutes.

55 checks in all; the full run takes about 1 hour 40 minutes. Numbers in
brackets (`[12]`) are the old numbers, kept for one release; "new" marks
checks added for the latest work.

Each check gives the exact command or keys, what you should see, and where to
look. Fill in the results table at the end, and record every difference from
what a check says in `docs/PARITY.md` (the row of that feature, or a new
row).

## Before you start

You need:
- `kodact` built from this repository and on `PATH`
  (`cargo build --release`, then `export PATH="$PWD/target/release:$PATH"`);
- a key: `export KODACT_API_KEY=...`;
- `git`, `python3` and `jq`.

Create the scratch repository (it refuses a non-empty directory unless given
`--force`):

```sh
scripts/check-setup.sh /tmp/kodact-check
cd /tmp/kodact-check
```

It holds:
- `calc.py`, whose committed `add` is right, with the bug `return a - b`
  uncommitted, and `test_calc.py` (`python3 test_calc.py` prints `ok` or
  fails);
- the branch `planted-bug`, which commits the same bug;
- `notes.md` (for `@` mentions) and `secret.txt` (git-ignored, for a deny
  rule);
- `.kodact/settings.json` with a `SessionStart` hook (`true`), so the hooks
  screen has something to list;
- `other-repo/`, a second repository for `/cd`.

**Each part starts from a fresh setup:** `scripts/check-setup.sh
/tmp/kodact-check --force`. Inside a part, the checks run in order and some
use what an earlier one left.

Where to look:
- **The conversation:** `/export` (interactive) prints it as text: prompts,
  answers and one line per tool call (`› Read(...)`).
- **Requests:** `/debug` turns on logging and prints the log's path; each
  request is a line with `"model request"`.
- **Files:** `git diff`, `git status`, `cat`.

## A. Print mode

1. [1] `kodact -p "/goal python3 test_calc.py prints ok" --output-format stream-json --verbose`
   - KodAct fixes `calc.py`, runs the test, and stops.
   - The last `system` line with `"subtype": "goal"` has
     `"status": "achieved"`; `echo $?` prints `0`.
2. [2] Put the bug back (`printf 'def add(a, b):\n    return a - b\n' > calc.py`),
   then `kodact -p "/goal make the tests pass without touching any file"`.
   - stderr has `Goal can't be met: ...`; `echo $?` prints `1`.
3. [26, part] `kodact -p --permission-mode acceptEdits "fix @calc.py"`
   - `git diff` shows no change left in `calc.py` (`return a + b` again).
   - Run it again on the bug with `--output-format stream-json --verbose`
     added: there is no `"name":"Read"` tool call before the `Edit`, because
     the file came attached.
4. (new) `kodact -p "what does @notes.md say about the CLI?"`
   - The answer says Monday, without reading the file first
     (with `--output-format stream-json --verbose`,
     `grep -c '"name":"Read"'` prints `0`).
5. (new) `kodact -p "what does @secret.txt say?" --disallowedTools "Read(secret.txt)" --output-format stream-json --verbose > /tmp/kodact-deny.jsonl`
   - `grep -c 4417 /tmp/kodact-deny.jsonl` prints `0`: neither the attachment
     nor a Read call got the file.
   - The answer says the file can't be read (the prompt carried the note
     `secret.txt was not attached: blocked by a permission rule; use Read`).
6. [18] `git stash -q && git checkout -q planted-bug`, then
   `kodact -p "/code-review high"`.
   - The report names `calc.py:2` with an input that fails (for example
     `add(2, 3)` returns `-1`).
   - Afterwards: `git checkout -q - && git stash pop -q`.
7. (new) A stream-json session with commands during a long turn:

   ```sh
   user() { jq -cn --arg t "$1" '{type: "user", message: {role: "user", content: $t}}'; }
   { user "Explain every file in this repository in detail, one by one."; sleep 8
     user "/usage"; user "/btw what are you working on?"; sleep 60; } |
   kodact -p --input-format stream-json --output-format stream-json --verbose |
   jq -c 'select(.type == "result") | {immediate, num_turns, result: (.result // "" | .[0:70])}'
   ```

   - The first two result lines have `"immediate": true` and
     `"num_turns": 0`: the `/usage` table (model calls above 0) and the side
     answer. They come before the long turn's result, which has
     `"immediate": null` and `num_turns` of 1 or more.
   - Raise the second `sleep` if the turn is still running when it ends.
8. (new) The same with `user "/status"` instead of the two commands: its
   result line names the model and comes before the turn's.
9. [14, part] `kodact -p "/status"` prints `Directory:` as
   `/tmp/kodact-check`, the model and `Permissions:    default mode`.
10. (new) `kodact -p "/hooks add PreToolUse Bash echo hook-ran >> /tmp/kodact-hook.log"`
    - It says `Added a PreToolUse hook for Bash`; `.kodact/settings.local.json`
      has it.
    - `kodact -p --permission-mode acceptEdits "run ls with Bash"`, then
      `cat /tmp/kodact-hook.log` shows `hook-ran`.
    - `kodact -p "/hooks"` numbers it (`1. [Bash] echo hook-ran ... local`);
      `kodact -p "/hooks remove PreToolUse 1"` removes it from the file.
11. (new) `kodact -p "/agents create test-writer --description 'Writes unit tests' --tools Read,Write,Bash --model sonnet"`
    - `.kodact/agents/test-writer.md` has those fields.
    - `kodact -p --permission-mode acceptEdits "use the test-writer agent to add a test for add(0, 0)" --output-format stream-json --verbose`
      shows a `"name":"Task"` call with `"subagent_type":"test-writer"`.
12. [17] `kodact -p "/feedback checklist run"`
    - It names a bundle directory; `grep -r sk- <bundle>` finds nothing.

## B. Interactive (a real terminal emulator)

Reset first (`scripts/check-setup.sh /tmp/kodact-check --force`), then start
`kodact` in `/tmp/kodact-check`. Unless a check says otherwise, keep that
session open from one check to the next.

### The terminal UI

13. [19] The prompt box and the status line appear at the bottom. `hello`
    streams an answer; scrolling up with the mouse or Shift+PageUp shows the
    whole conversation.
14. [20] Ask for something long (`explain every file here in detail`) and
    press Esc while it streams. The turn stops; `/export` shows
    `[Request interrupted by user]`.
15. [21] Type a second message while a turn runs. It shows as queued (`⏎`)
    and is sent when the turn ends.
16. [22] `run git status with Bash` in default mode. The permission dialog
    appears; option 2 adds the rule (`/permissions` lists it).
17. [23] Shift+Tab twice shows plan mode in the status line.
    `make add subtract instead` ends with the plan dialog; option 1 switches
    to accept edits.
18. [25] Leave with `/exit`; start `kodact` again and leave with Ctrl-D;
    again, with Ctrl-C twice. Each time the shell works afterwards: typed
    text echoes, and `stty -a` shows `icanon echo`.
19. (new, gap) `/terminal-setup` says whether the keyboard protocol is on in
    this terminal, and that matches what Shift+Enter does here (a new line,
    or a send).
20. (new, gap) `/copy` after an answer: paste into another program; it is
    the answer's text. (Inside tmux: check 50.)
20a. (new) `add an age check to the signup form, with tests` with
    `KODACT_NUDGES=todoTracking`. Each plan update shows the plan as a
    checklist under `Updated the plan 1 of 4 done`: done tasks with a green
    `✓` and struck through, the one in progress in lime and bold with a
    still `◉`, the rest `○`. While the model works, the spinner names the
    task in progress (`Adding the age field…`), and at 140 columns or more
    the sidebar's Plan pane shows that task with a lime ring of dots whose
    gap goes round its edge, clockwise, whole and still once the turn ends;
    the steps' text stays lined up. Check it in a dark and a light terminal.

### Goals and loops

21. [3] `/loop 2m say the time`
    - It answers at once, then about every 2 minutes (up to a minute late).
    - `/tasks` lists it as `every 2 minutes`; `/tasks stop <id>` ends it.
22. [4] `/loop watch for a file named done.txt and stop when it exists`;
    a few minutes later, in another shell, `touch /tmp/kodact-check/done.txt`.
    - The model schedules its own checks with ScheduleWakeup.
    - Once the file exists, it stops the loop (`The loop is finished.`).

### Context and history

23. [5] `add a docstring to add`, then `rename add to plus` (allow the
    edits).
    - `/rewind` lists both prompts.
    - `/rewind 2 code` restores `calc.py` to before the rename; `git diff`
      shows only the docstring (and the subtraction).
24. [6] `/btw what file did you edit last?`
    - It answers with the file name.
    - `/context all` shows the same message count as before the question.
25. [7] `/rewind 2 summarize-to` replaces the first prompt's turn with a
    summary. `/export` shows the summary note, then prompt 2.
26. [26] `/clear`, then `explain @calc.py`.
    - It answers from the file without a Read call (`/export` has no
      `› Read` line).
    - A dim `(attached: calc.py)` line follows the prompt; `/rewind` lists the
      prompt as typed.

### Immediate commands

27. [27] Ask for something long (`explain every file here in detail`).
    While it streams, type `/usage`.
    - The answer appears at once; the spinner keeps going and the turn isn't
      interrupted.
    - It counts the turn so far (model calls above 0 once the first reply
      came, a cost above $0). `/usage` after the turn shows the final totals.
28. [28] During another long turn, `/btw what are you working on?`.
    - It answers in a line or two while the turn goes on.
    - After the turn, `/usage` includes the side question's tokens, and
      `/export` has no trace of it.
29. (new) During a long turn, `/status` and `/tasks` answer at once too;
    `/compact` waits (it shows as queued).

### Settings and modes

30. [8] `/fast on` with the default model, then `/debug`, then a question.
    - The log (its path is in `/debug`'s answer) has `speed: fast` on the
      `"model request"` line.
    - `/model haiku`, then `/fast on`: it refuses with
      `Fast mode isn't available for claude-haiku-4-5.`
31. [9] `/effort low`, then a question.
    - The `/debug` log shows `output_config` with `"effort": "low"`.
    - `<config>/settings.json` (`kodact config paths`) has
      `"effortLevel": "low"`.
32. [10] `/output-style explanatory`, then `how should I test calc.py?`.
    The answer includes the explanatory style's insight notes.

### Sessions

33. [12] `/clear first try`, then `/resume`. The picker shows "first try";
    choosing it brings it back with its messages.
34. [13] `/branch experiment`, then `add a comment to calc.py`. `/resume`
    and pick the original: it doesn't have that change.
35. [14] `/cd other-repo` keeps the conversation. `/status` shows
    `/tmp/kodact-check/other-repo`, and the next answer knows about the move.
    `/cd ..` to come back.

### Extras

36. [15] `/advisor opus`, then
    `refactor calc.py into a class and make sure nothing breaks`.
    - KodAct calls the Advisor tool at least once before finishing (`/export`
      shows `› Advisor`).
    - `/usage` shows the advisor model's cost.

### Screens

Reset first, so `calc.py` has its uncommitted change.

37. [29] `/diff` opens a box listing `calc.py  +1 -1`.
    - Enter on `calc.py` jumps to its hunk: the addition green, the deletion
      red, the `@@` line dim. Esc goes back to the list; Esc again closes it.
    - PageDown/PageUp and Home/End scroll; the footer counts the rows.
38. [30] `/context` shows a 10×10 grid coloured by part; the legend's
    numbers match `/context all`'s top lines. During a long turn, `/context`
    still opens.
39. [31] `/hooks`: Enter on "Add hook…", Right to `PreToolUse`, Tab, `Bash`,
    Tab, `echo hook-ran >> /tmp/kodact-hook.log`, Tab, `local`, Enter.
    - The reply says it was added and applies now;
      `.kodact/settings.local.json` has it. The `SessionStart` hook from the
      setup is listed too, with source `project`.
    - `run ls with Bash`: `/tmp/kodact-hook.log` gets a line.
    - `/hooks` again, Enter on the PreToolUse hook, Yes: it is gone from the
      file.
40. [32] `/agents`: Enter on "Create an agent…", name `test-writer`, a
    description, Space on Read and Write, model sonnet, project, Enter.
    - `.kodact/agents/test-writer.md` exists with those fields.
    - `/agents` lists it; `use the test-writer agent to add a test` runs it
      through the Task tool.
41. [34] `/color green` turns the prompt marker and selections green;
    `/color default` brings KodAct's colour back.
42. [34] `/focus`, then `run ls and git status`: no tool lines in the
    scrollback, the spinner names each tool; `/focus off` shows them again.
43. [33] Leave, write
    `{"ctrl+s": "submit", "ctrl+r": "none", "ctrl+q": "nope"}` to
    `keybindings.json` in the config directory (`kodact config paths`), and
    start `kodact`.
    - A warning names `ctrl+q` and the unknown action.
    - Ctrl+S sends a prompt; Ctrl+R does nothing.
    - `/keybindings` lists `enter, ctrl+s` for sending and `Unbound: ctrl+r.`
    - Delete the file afterwards.

### Abnormal exits (a known gap)

44. (new, gap) Start `kodact`; from another shell, `pkill -TERM -x kodact`.
    The shell works afterwards (`stty -a` shows `icanon echo`; typed text
    echoes). If it doesn't, `reset` repairs it: record that in PARITY.
45. (new, gap) Start `kodact` and close the terminal window (or tab) while a
    turn runs. In a new one, `ps aux | grep '[f]orge'` shows no leftover
    process.
46. (new, gap) A panic can't be forced from outside, so the path that
    restores the terminal after one is covered only by reading the code
    (`crates/kodact-cli/src/tui/mod.rs`: the panic hook installed in `setup()`
    calls `restore()`, and so does `Guard`'s `Drop`) and by `tui::tests`.
    Record it as "read, not run".

## C. Special setups

47. [24] `NO_COLOR=1 kodact`: no colours anywhere (markers and selections
    are bold or reversed). `/diff` still marks lines with `+` and `-`;
    `/context`'s grid uses letters (`S`, `T`, `G`, `·`).
48. [24] In a 40-column window: the input box wraps without breaking its
    border; `/diff` and `/hooks` fit inside the window.
49. (new, gap) Shift+Enter in a terminal with the keyboard protocol (kitty,
    WezTerm, Ghostty or foot) starts a new line instead of sending.
    `/terminal-setup` says the protocol is on.
50. (new, gap) In tmux with `set -g set-clipboard on` (and, for Shift+Enter,
    `set -s extended-keys on`): `/copy` reaches the system clipboard (paste
    outside tmux).
51. (new, gap) Over SSH (`ssh localhost`, then `kodact` in the scratch
    repository): the UI draws, Esc interrupts, and `/copy` reaches the local
    clipboard where the terminal supports OSC 52.
52. [11] On Linux with bubblewrap: `/sandbox on`, then
    `touch /etc/kodact-check with Bash`. The command fails inside the
    sandbox, and `/sandbox` says it is on.
53. [16] On a machine with MCP servers in `~/.codex/config.toml`: `/import`
    lists them; `/import codex --yes` adds them to `<config>/settings.json`;
    the next session connects them (`/mcp`).
54. (new) The line REPL: `kodact --no-tui`. `hello` answers; `/status` typed
    during a turn runs after it (the REPL reads input only between turns).
55. (new) `KODACT_TUI=0 kodact` also starts the line REPL.

### Windows (Windows Terminal and PowerShell)

Checked under Wine with Git for Windows 2.47 (not a real Windows machine):
the shell search, the no-shell message, `kodact doctor` and paths without
`\\?\`. These need a real Windows machine:

56. (new) With Git for Windows installed: `kodact doctor` shows
    `ok   shell        Git Bash (C:\Program Files\Git\bin\bash.exe), ...`
    and `ok   git`. In `kodact`, `run echo hello` answers `hello`; the header
    and `/status` show `C:\...` paths, never `\\?\C:\...`.
57. (new) Without Git Bash (or with `KODACT_SHELL=C:\nowhere\bash.exe`):
    `kodact doctor` fails the `shell` row and names what it looked for; in
    `kodact`, `run echo hello` gets one clear answer naming the fix, and the
    model does not retry through cmd.exe or PowerShell.
58. (new) `KODACT_SHELL=pwsh kodact`: `run Get-ChildItem` works, and `/status`
    shows `Shell: PowerShell 7 (...)`.
59. (new) `run ping -n 100 127.0.0.1`, then Esc after a few seconds: the
    command stops at once (Task Manager shows no leftover `PING.EXE`).
60. (new) A deny rule `Read(./secrets/**)`: reading `.\Secrets\key.txt` or
    `/c/.../secrets/key.txt` is denied too.
61. (new) An OpenAI-compatible gateway with only `KODACT_API_KEY` set: `kodact`
    warns that `KODACT_OPENAI_API_KEY` is not set at startup; the 401 names
    `KODACT_OPENAI_API_KEY`; `kodact doctor` fails the `credentials` row;
    `kodact doctor --probe` with the right key says the key works.

## Results

Copy this table into the release notes or an issue, one row per check:

| Check | Pass / fail / skipped | Notes | KodAct version (`kodact --version`) |
| --- | --- | --- | --- |
| 1 | | | |
| ... | | | |
| 55 | | | |

Record every failure, and every behaviour that differs from the reference,
in `docs/PARITY.md`: the feature's row (its status and notes), or a new row
if none fits. A check that couldn't be run (no tmux, no bubblewrap) is
"skipped", with the reason.
