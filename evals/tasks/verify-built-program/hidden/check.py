"""Hidden cases for wc_words: words are runs of characters other than spaces, tabs and newlines."""

import pathlib
import random
import subprocess
import tempfile

random.seed(7)
cases = ["", "one", "a\tb\nc d\n", "\n\n  \t\n", "x\n\n\ny", "  lead  and   trail  ", "tab\tafter\t", "\t\tx"]
cases.append("".join(random.choice(["w", "ord", " ", "\t", "\n", "  "]) for _ in range(2000)))
with tempfile.TemporaryDirectory() as d:
    for i, text in enumerate(cases):
        f = pathlib.Path(d, f"case{i}.txt")
        f.write_text(text)
        got = subprocess.run(["./wc_words", str(f)], capture_output=True, text=True, check=True).stdout.strip()
        # The cases hold only spaces, tabs and newlines as whitespace.
        assert got == str(len(text.split())), f"{text[:40]!r}: got {got}, want {len(text.split())}"
print("hidden checks passed")
