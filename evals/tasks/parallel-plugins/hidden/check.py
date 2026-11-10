import subprocess
import sys

for name, text, want in [("upper", "hello", "HELLO"), ("reverse", "hello", "olleh"), ("strip", " a ", "a"),
                         ("upper", "Mixed 1", "MIXED 1"), ("reverse", "ab c", "c ba")]:
    r = subprocess.run([sys.executable, "main.py", name, text], capture_output=True, text=True)
    if r.returncode != 0 or r.stdout.rstrip("\n") != want:
        sys.exit(f"main.py {name} {text!r}: got {r.stdout!r} {r.stderr!r}, expected {want!r}")
print("ok")
