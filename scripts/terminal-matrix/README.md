# The terminal matrix

The pre-production check of KodAct's terminal (`docs/PREPRODUCTION_TEST.md`),
as one command. It drives the real `kodact` binary the way a person does and
reads the screen; a unit test passing is not what it checks.

```bash
pip install pyte            # the terminal emulator the screen is read through
cargo build -p kodact-cli
scripts/terminal-matrix/run.py
```

About 35 minutes on 4 cores. It exits 1 when any row failed and writes the
ledger to `target/terminal-matrix/ledger.md` (and `.json`): one row per thing
checked, with what was seen when it failed.

```bash
scripts/terminal-matrix/run.py --list                # every check
scripts/terminal-matrix/run.py -s B                  # one section (A to G)
scripts/terminal-matrix/run.py -k clear -d inline    # by name, one display
scripts/terminal-matrix/run.py -s B -x "every form"  # a section without its longest checks
scripts/terminal-matrix/run.py -k SIGTERM -v         # print what a failing row saw
KODACT_BIN=target/release/kodact scripts/terminal-matrix/run.py
```

## What it is made of

| File | What it is |
| --- | --- |
| `kterm.py` | `Term`: `kodact` in a pseudo-terminal, read through pyte (the screen, the cursor, the terminal modes left on, the scrollback). `Stub`: a local OpenAI-compatible endpoint (also the Messages API and Gemini paths) that plays the model from scripts: text, tool calls, thinking, slow replies, 401/429/500, a dropped connection, a malformed stream. `Sandbox`: a throwaway home, state folder and git project |
| `run.py` | The runner and the ledger. A check calls `c.ok(condition, what, saw)` for each thing it looks at; `c.known(..)` records a fault seen and left, `c.notrun(..)` one that could not be run here |
| `checks/a_start.py` … `g_safety.py` | The matrix, a section per file. Section B takes the commands from `kodact_core::commands::BUILTINS` (read from the source), so a command added later is covered |
| `mcp_stub.py` | A small MCP server on stdio, with a tool that kills it |

## What is stubbed, and what is not

- The model is the stub endpoint. Nothing else is: the binary, its sessions,
  settings, hooks, MCP client, shell and permission engine are the real ones.
- The terminal is pyte, an emulator. It does not draw joined emoji as one
  glyph and has no alternate screen of its own; the checks that depend on
  either say so in the ledger. tmux is driven for real when installed.

## Adding a check

```python
@check("D", "what a person does, in a few words")
def my_check(c):
    sb, st, t = c.session()                  # a sandbox, a stub and kodact at its front page
    st.rules["do-it"] = [{"text": "Doing.", "tools": [("Bash", {"command": "echo hi"})]}, {"text": "done-mark"}]
    t.type("do-it", settle=False)
    t.wait("Allow once")                     # the screen, not the answer's text
    t.send("y")
    t.wait("done-mark")
    c.ok(os.path.exists(...), "the file is there", os.listdir(sb.project))   # the state, not the message
    c.ok(c.quit(t) == 0 and not t.left_clean(), "/exit leaves the terminal as it was", t.left_clean())
```

A prompt containing `ping-<tag>` is always echoed by the stub: `c.alive(t, tag)`
uses it to ask "does the session still answer?".
