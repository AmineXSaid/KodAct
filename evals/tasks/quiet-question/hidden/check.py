import sys

try:
    got = float(open("answer.txt").read().strip())
except (OSError, ValueError) as e:
    sys.exit(f"answer.txt: {e}")
if abs(got - 11.5) > 1e-9:
    sys.exit(f"answer {got}, want 11.5")
print("ok")
