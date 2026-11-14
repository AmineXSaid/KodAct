import subprocess
import sys

sys.path.insert(0, "src")
from members import parse_rows  # noqa: E402

cases = [
    (["1,Ann", "2, Bo"], [(1, "Ann"), (2, "Bo")]),
    ([",Ann", "2,Bo"], [(2, "Bo")]),
    (["  ,Ann", "", "2,Bo", "2,Cy", "3,Di", "3,Ed"], [(2, "Bo"), (3, "Di")]),
    (["7,Ann", "7,Bo", ",Cy", "8,Di"], [(7, "Ann"), (8, "Di")]),
]
for lines, want in cases:
    got = parse_rows(lines)
    if got != want:
        sys.exit(f"parse_rows({lines}) is {got}, not {want}")
# The request names members.py alone (it is src/members.py) and joins words with slashes
# ("and/or", "missing/duplicate"): none of them is a file to make. Made all the same, they are
# what a report of missing files led to.
status = subprocess.run(
    ["git", "status", "--porcelain", "--untracked-files=all"], capture_output=True, text=True
).stdout.splitlines()
made = [line[3:] for line in status if line.startswith("??")]
wrong = [p for p in made if p == "members.py" or p.split("/")[0] in ("and", "or", "missing", "duplicate")]
if wrong:
    sys.exit("files made that the request never asked for: " + ", ".join(wrong))
print("ok")
