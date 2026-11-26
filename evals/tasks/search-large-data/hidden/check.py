import os
import sys

want = open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "hidden", "expected.txt")).read().strip()
try:
    got = open("answer.txt").read().strip()
except FileNotFoundError:
    sys.exit("answer.txt is missing")
if got != want:
    sys.exit(f"answer.txt holds {got!r}, want {want!r}")
print("ok")
