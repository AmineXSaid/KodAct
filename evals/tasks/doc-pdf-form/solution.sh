python3 - <<'PY'
import json
from pypdf import PdfReader, PdfWriter
from pypdf.generic import NameObject

d = json.load(open("details.json"))
w = PdfWriter(clone_from="application.pdf")
vals = {k: d[k] for k in ("applicant", "organisation", "date", "amount", "signatory")}
for page in w.pages:
    w.update_page_form_field_values(page, vals, auto_regenerate=False)
for page in w.pages:
    for a in page.get("/Annots", []) or []:
        a = a.get_object()
        if a.get("/T") == "matched":
            on = [k for k in a["/AP"]["/N"].keys() if k != "/Off"][0]
            a[NameObject("/V")] = NameObject(on)
            a[NameObject("/AS")] = NameObject(on)
w.set_need_appearances_writer(True)
for p in PdfReader("appendix.pdf").pages:
    w.add_page(p)
w.write("application-filled.pdf")
PY
