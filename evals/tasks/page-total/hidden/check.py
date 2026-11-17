"""page-total: the total is what the page shows after its script ran, read from the DOM a
headless Chrome renders; computed, not written into the script."""
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


html = dom("index.html")
m = re.search(r'id="total">([^<]*)<', html)
if not m:
    sys.exit("no #total on the rendered page")
if m.group(1).strip() != "47.00":
    sys.exit(f"the page shows the total as {m.group(1).strip()!r}, want 47.00")
if len(re.findall(r"<tr>", html)) != 3:
    sys.exit("the page no longer lists the three items")
if "47" in open("app.js").read():
    sys.exit("the total is written into app.js, not computed")
print("ok")
