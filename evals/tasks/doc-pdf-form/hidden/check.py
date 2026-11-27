"""application-filled.pdf: the form's fields filled from details.json (the
checkbox ticked), still showing its values when opened, with appendix.pdf's
three pages after the form's two."""

import json
import os
import sys

import pdfplumber
import pypdf

if not os.path.exists("application-filled.pdf"):
    sys.exit("application-filled.pdf was not made")
try:
    r = pypdf.PdfReader("application-filled.pdf")
except Exception as e:
    sys.exit(f"application-filled.pdf does not open: {e}")
if len(r.pages) != 5:
    sys.exit(f"want the form's 2 pages and the appendix's 3, got {len(r.pages)} pages")
with pdfplumber.open("application-filled.pdf") as doc:
    tail = [(p.extract_text() or "") for p in doc.pages[2:]]
if not all("Appendix" in t for t in tail):
    sys.exit("pages 3-5 aren't the appendix, in order")

want = json.load(open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "repo", "details.json")))
fields = r.get_fields() or {}
text_fields = {k: want[k] for k in ("applicant", "organisation", "date", "amount", "signatory")}
if fields:
    for k, v in text_fields.items():
        got = str((fields.get(k) or {}).get("/V", "")).strip()
        if got != v:
            sys.exit(f"field {k} is {got!r}, want {v!r}")
    box = str((fields.get("matched") or {}).get("/V", "/Off"))
    if box in ("/Off", "", "None"):
        sys.exit("the matched-funding box isn't ticked")
    # Values a viewer shows: each filled widget has an appearance, or the
    # form asks viewers to draw them.
    acro = r.trailer["/Root"].get("/AcroForm", {})
    need = bool(acro.get("/NeedAppearances", False))
    for page in r.pages[:2]:
        for a in page.get("/Annots", []) or []:
            a = a.get_object()
            if a.get("/FT") == "/Tx" and a.get("/V") and not need and "/AP" not in a:
                sys.exit(f"field {a.get('/T')} has a value but no appearance: viewers may show it empty")
else:
    # Flattened: the values are part of the page.
    with pdfplumber.open("application-filled.pdf") as doc:
        text = " ".join((p.extract_text() or "") for p in doc.pages[:2])
    for k, v in text_fields.items():
        if v not in text:
            sys.exit(f"the flattened form doesn't show {k} = {v!r}")
print("ok")
