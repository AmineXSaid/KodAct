"""lib/wordcount/counter.py counts words, its test sits beside the other tests and the suite
passes; and KodAct didn't report as missing a file that the run made.

The request names the module both by its path and alone ("counter.py"), and the test file alone
("test_counter.py"), to go where the project keeps its tests. Read as files of their own at the
project's root, both were reported missing in the run's result (`verification.missing`) after
the model had made them where the request said. kodact-eval writes the run's stream next to the
workspace (transcript.jsonl); without one (task validation), only the files are checked.
"""

import importlib.util
import json
import os
import subprocess
import sys

path = os.path.join("lib", "wordcount", "counter.py")
if not os.path.exists(path):
    sys.exit(f"{path} is missing")
spec = importlib.util.spec_from_file_location("counter", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for text, want in [("", 0), ("   \n\t ", 0), ("one", 1), ("two words", 2), ("tabs\tand\nnew lines", 4)]:
    got = module.count_words(text)
    if got != want:
        sys.exit(f"count_words({text!r}) is {got!r}, not {want}")
if not os.path.exists(os.path.join("tests", "test_counter.py")):
    sys.exit("tests/test_counter.py is missing: the request asks for it next to the existing tests")
run = subprocess.run([sys.executable, "-m", "unittest", "discover", "-s", "tests"], capture_output=True, text=True)
if run.returncode != 0:
    sys.exit("the tests fail:\n" + run.stderr[-2000:])

stream = os.path.join(os.path.dirname(os.getcwd()), "transcript.jsonl")
if not os.path.exists(stream):
    sys.exit(0)
end = None
for line in open(stream, encoding="utf-8", errors="replace"):
    try:
        ev = json.loads(line)
    except ValueError:
        continue  # a line cut short when the run timed out
    if isinstance(ev, dict) and ev.get("type") == "result":
        end = ev
missing = ((end or {}).get("verification") or {}).get("missing") or []
if missing:
    sys.exit("KodAct reported as missing files the run made: " + ", ".join(missing))
print("ok")
