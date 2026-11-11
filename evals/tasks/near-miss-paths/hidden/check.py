import os
import sys

want = {
    "src/config/Network.py": "TIMEOUT_SECONDS = 30\nPROXY = None\n",
    "src/config/retry_policy.py": "RETRIES = 4\nBACKOFF_SECONDS = 2\n",
}
for path, text in want.items():
    if open(path).read() != text:
        sys.exit(f"{path} is not as asked: {open(path).read()!r}")
extra = [os.path.join(d, f) for d, _, fs in os.walk("src") for f in fs if "settings" in d or f in ("network.py", "Retry.py")]
if extra:
    sys.exit(f"new files were made instead of changing the ones there: {extra}")
print("ok")
