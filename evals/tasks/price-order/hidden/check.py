"""price-order: the order is what the page shows after its script ran, read from the DOM a
headless Chrome renders."""
import os
import re
import shutil
import subprocess
import sys


def chrome():
    if os.environ.get("KODACT_EVAL_CHROME"):
        return os.environ["KODACT_EVAL_CHROME"]
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome"):
        if shutil.which(name):
            return shutil.which(name)
    sys.exit("no headless Chrome or Chromium (KODACT_EVAL_CHROME)")


def dom(page):
    r = subprocess.run(
        [chrome(), "--headless=new", "--no-sandbox", "--disable-gpu", "--allow-file-access-from-files",
         "--virtual-time-budget=2000", "--dump-dom", "file://" + os.path.abspath(page)],
        capture_output=True, text=True, timeout=60,
    )
    return r.stdout


html = dom("prices.html")
names = re.findall(r"<li>([^:<]*):", html)
want = ["Cable ties", "Mouse pad", "Desk lamp", "Webcam", "Monitor stand"]
if names != want:
    sys.exit(f"the page lists {names}, want {want}")
print("ok")
