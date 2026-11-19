"""Split a flags setting (`KODACT_HARBOR_ARGS`, `PI_HARBOR_ARGS`...) into words as bash would.

`shlex.split` knows '…', "…" and backslashes, but not bash's `$'…'` quoting, which is what
`printf '%q'` writes for text with a newline or a control character. A skill quoted that way
(`KODACT_HARBOR_ARGS="--append-system-prompt $(printf '%q' "$SKILL_TEXT")"`) failed with "No
closing quotation" on its first apostrophe (2026-11-19). This reads all four forms. It quotes
only: nothing is expanded, and a `$(…)` or a backquote left in the setting is refused, since
it would reach kodact as text, not as what the command prints (job …-163009 appended the word
`$(cat` as its system prompt).

    python3 evals/harbor/shell_args.py   # checks $KODACT_HARBOR_ARGS, as run.sh does
"""

import os
import sys

BLANK = " \t\n"
# bash's one-letter escapes in $'…'.
SIMPLE = {"a": "\a", "b": "\b", "e": "\x1b", "E": "\x1b", "f": "\f", "n": "\n", "r": "\r",
          "t": "\t", "v": "\v", "\\": "\\", "'": "'", '"': '"', "?": "?"}
HEX = "0123456789abcdefABCDEF"


def _ansi_c(s: str, i: int) -> tuple[str, int]:
    """The text of the $'…' whose content starts at s[i], and the index just past its closing quote."""
    out = []
    while i < len(s):
        c = s[i]
        if c == "'":
            return "".join(out), i + 1
        if c != "\\" or i + 1 == len(s):
            out.append(c)
            i += 1
            continue
        e = s[i + 1]
        i += 2
        if e in SIMPLE:
            out.append(SIMPLE[e])
        elif e in "01234567":
            j = i - 1
            while i < len(s) and i - j < 3 and s[i] in "01234567":
                i += 1
            out.append(chr(int(s[j:i], 8) & 0xFF))
        elif e in "xuU":
            width = {"x": 2, "u": 4, "U": 8}[e]
            j = i
            while i < len(s) and i - j < width and s[i] in HEX:
                i += 1
            out.append(chr(int(s[j:i], 16)) if i > j else "\\" + e)
        elif e == "c" and i < len(s):
            out.append(chr(ord(s[i]) & 0x1F))
            i += 1
        else:
            out.append("\\" + e)
    raise ValueError("No closing quotation")


def _refuse_substitution(s: str, i: int) -> None:
    if s.startswith("$(", i) or s[i] == "`":
        raise ValueError(
            f"a command substitution ({s[i:i + 12]}…) is not run here: run it in your own shell, "
            "outside single quotes, so its output is what the setting holds")


def split(s: str) -> list[str]:
    """The words of s as bash would split them: '…', "…", $'…', backslashes; no expansion."""
    words: list[str] = []
    word: list[str] | None = None
    i = 0
    while i < len(s):
        c = s[i]
        if c in BLANK:
            if word is not None:
                words.append("".join(word))
                word = None
            i += 1
            continue
        if word is None:
            word = []
        if c == "'":
            j = s.find("'", i + 1)
            if j < 0:
                raise ValueError("No closing quotation")
            word.append(s[i + 1:j])
            i = j + 1
        elif c == "$" and s.startswith("'", i + 1):
            text, i = _ansi_c(s, i + 2)
            word.append(text)
        elif c == '"':
            i += 1
            while True:
                if i >= len(s):
                    raise ValueError("No closing quotation")
                c = s[i]
                if c == '"':
                    i += 1
                    break
                _refuse_substitution(s, i)
                if c == "\\" and i + 1 < len(s) and s[i + 1] in '$`"\\\n':
                    if s[i + 1] != "\n":
                        word.append(s[i + 1])
                    i += 2
                else:
                    word.append(c)
                    i += 1
        elif c == "\\":
            if i + 1 < len(s) and s[i + 1] != "\n":
                word.append(s[i + 1])
            i += 2
        else:
            _refuse_substitution(s, i)
            word.append(c)
            i += 1
    if word is not None:
        words.append("".join(word))
    return words


if __name__ == "__main__":
    name = sys.argv[1] if len(sys.argv) > 1 else "KODACT_HARBOR_ARGS"
    try:
        split(os.environ.get(name, ""))
    except ValueError as e:
        print(e, file=sys.stderr)
        sys.exit(1)
