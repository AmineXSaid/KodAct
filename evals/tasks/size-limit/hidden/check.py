import os
import subprocess
import sys

size = os.path.getsize("roman.py")
if size >= 400:
    sys.exit(f"roman.py is {size} bytes; it must be under 400")


def run(arg):
    r = subprocess.run([sys.executable, "roman.py", arg], capture_output=True, text=True, timeout=30)
    return r.returncode, r.stdout.strip()


def roman(n):
    out = ""
    for v, s in zip([1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1],
                    "M CM D CD C XC L XL X IX V IV I".split()):
        while n >= v:
            out += s
            n -= v
    return out


for n in [1, 3, 4, 9, 14, 40, 44, 90, 400, 444, 1994, 2024, 3888, 3999]:
    if run(str(n)) != (0, roman(n)):
        sys.exit(f"python3 roman.py {n}: got {run(str(n))}, want (0, {roman(n)!r})")
    if run(roman(n)) != (0, str(n)):
        sys.exit(f"python3 roman.py {roman(n)}: got {run(roman(n))}, want (0, '{n}')")
for bad in ["0", "4000", "IIII", "VX", "abc", "MMMM", "IC"]:
    if run(bad) != (1, "invalid"):
        sys.exit(f"python3 roman.py {bad}: got {run(bad)}, want (1, 'invalid')")
print("ok")
