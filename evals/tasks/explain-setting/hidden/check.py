import sys
from pathlib import Path

try:
    answer = Path("answer.txt").read_text().strip()
except OSError:
    sys.exit("answer.txt is missing")
name, _, port = answer.partition("=")
if (name.strip(), port.strip()) != ("SHOP_HTTP_PORT", "8085"):
    sys.exit(f"answer.txt says {answer!r}")
print("answer is right")
