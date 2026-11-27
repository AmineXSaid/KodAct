"""incident-report.docx: real heading styles (so it has an outline and a
navigation pane), the incidents table, the actions as Word's own numbered list
(not typed numbers), page numbers in the footer, two pages at most."""

import csv
import os
import re
import subprocess
import sys
import tempfile
import zipfile

import docx

if not os.path.exists("incident-report.docx"):
    sys.exit("incident-report.docx was not made")
try:
    d = docx.Document("incident-report.docx")
except Exception as e:
    sys.exit(f"incident-report.docx does not open: {e}")

heads = [p for p in d.paragraphs if p.style.name.startswith("Heading") or p.style.name == "Title"]
if len([p for p in heads if p.style.name.startswith("Heading")]) < 3:
    sys.exit(f"want at least 3 headings in Word's Heading styles (an outline), got {[p.style.name for p in heads]}")

rows = list(csv.DictReader(open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "incidents.csv"))))
table = None
for t in d.tables:
    cells = " ".join(c.text for r in t.rows for c in r.cells)
    if all(r["team"] in cells for r in rows):
        table = t
if table is None:
    sys.exit("no table with every team from incidents.csv")
if len(table.rows) < len(rows) + 1:
    sys.exit(f"the table has {len(table.rows)} rows; want a header and {len(rows)} teams")
for r in rows:
    line = next((x for x in table.rows if any(c.text.strip() == r["team"] for c in x.cells)), None)
    vals = [c.text.strip() for c in line.cells] if line else []
    if r["incidents"] not in vals:
        sys.exit(f"{r['team']}'s row should show {r['incidents']} incidents: {vals}")


def numbered(p):
    ppr = p._p.pPr
    if ppr is not None and ppr.numPr is not None:
        return True
    s = p.style
    while s is not None:
        sp = s.element.pPr
        if sp is not None and sp.numPr is not None:
            return True
        s = s.base_style
    return False


actions = [p for p in d.paragraphs if numbered(p) and p.text.strip()]
if len(actions) < 4:
    sys.exit(f"want the four actions as a real numbered list, found {len(actions)} list paragraphs")
if any(re.match(r"\s*\d+[.)]\s", p.text) for p in actions):
    sys.exit("list paragraphs carry typed numbers on top of Word's numbering")

z = zipfile.ZipFile("incident-report.docx")
footers = " ".join(z.read(n).decode("utf8", "replace") for n in z.namelist() if re.match(r"word/footer\d*\.xml", n))
if "PAGE" not in footers:
    sys.exit("no page number field in the footer")

with tempfile.TemporaryDirectory() as tmp:
    subprocess.run(
        ["soffice", f"-env:UserInstallation=file://{tmp}/profile", "--headless", "--convert-to", "pdf", "--outdir", tmp, "incident-report.docx"],
        capture_output=True, timeout=180,
    )
    pdf = os.path.join(tmp, "incident-report.pdf")
    if not os.path.exists(pdf):
        sys.exit("the report does not render")
    import pypdf

    n = len(pypdf.PdfReader(pdf).pages)
    if n > 2:
        sys.exit(f"the brief asks for two pages at most; it renders to {n}")
print("ok")
