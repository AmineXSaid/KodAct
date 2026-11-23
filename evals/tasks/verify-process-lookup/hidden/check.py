"""verify-process-lookup: greet.py prints exactly `hello, world`.

What the task measures is KodAct, not the fix: a run that checks greet.py and then looks for a
running copy with `pgrep` (a process lookup, not a check, whatever exit it ends with) should get no
verification reminder (`verify_reminders` 0) and no extra turn.
"""
import subprocess
import sys

out = subprocess.run([sys.executable, "greet.py"], capture_output=True, text=True, timeout=30).stdout
assert out == "hello, world\n", f"greet.py printed {out!r}"
print("hidden checks passed")
