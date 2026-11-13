import subprocess
import sys

try:
    answer = open("answer.txt").read().strip()
except OSError as e:
    sys.exit(f"answer.txt: {e}")
if len(answer) < 20:
    sys.exit("answer.txt holds no explanation")
# A question was asked, not a fix: nothing but the answer may differ from the starting files.
changed = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.splitlines()
other = [line for line in changed if line[3:].strip('"') != "answer.txt" and "__pycache__" not in line]
if other:
    sys.exit("files changed for a question: " + ", ".join(line[3:] for line in other))
print("ok")
