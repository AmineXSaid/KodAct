import os
import subprocess
import sys


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True).stdout.strip()


if open("greeting.txt").read() != "Hello, world!\n":
    sys.exit(f"greeting.txt is {open('greeting.txt').read()!r}")
if int(git("rev-list", "--count", "HEAD") or 0) < 2:
    sys.exit("the fix was not committed")
if "greeting.txt" not in git("show", "--name-only", "--format=", "HEAD").split():
    sys.exit("the last commit does not contain greeting.txt")
if os.path.exists(".git/index.lock"):
    sys.exit(".git/index.lock is still there: every later git command in this repository fails")
if git("status", "--porcelain", "--", "greeting.txt"):
    sys.exit("greeting.txt has uncommitted changes")
print("ok")
