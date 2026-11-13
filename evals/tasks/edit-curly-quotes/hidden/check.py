import os
src = open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "strings.py"), encoding="utf-8").read()
expected = src.replace("10 MB", "25 MB", 1)
got = open("strings.py", encoding="utf-8").read()
assert got != src, "strings.py is unchanged"
assert got == expected, (
    "strings.py should differ from the original only in the limit (10 MB -> 25 MB), with the file's own "
    f"quotes, dashes and non-breaking space kept:\n{got!r}"
)
print("hidden checks passed")
