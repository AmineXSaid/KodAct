python3 - <<'PY'
import csv
import docx
from docx.oxml import OxmlElement
from docx.oxml.ns import qn

d = docx.Document()
d.add_heading("Q3 incident report", 0)
d.add_heading("Summary", 1)
d.add_paragraph("Mobile had the most incidents and the most downtime, after two bad releases in August. Identity had no Sev1 for the first time in a year.")
d.add_heading("Incidents by team", 1)
rows = list(csv.DictReader(open("incidents.csv")))
t = d.add_table(rows=1, cols=4)
t.style = "Light Grid Accent 1"
for c, h in zip(t.rows[0].cells, ["Team", "Incidents", "Sev1", "Hours down"]):
    c.text = h
trpr = t.rows[0]._tr.get_or_add_trPr()
trpr.append(OxmlElement("w:tblHeader"))
for r in rows:
    cells = t.add_row().cells
    for c, k in zip(cells, ["team", "incidents", "sev1", "hours_down"]):
        c.text = r[k]
d.add_heading("Actions", 1)
d.add_heading("Agreed with the leads", 2)
for a in ["Staged rollouts for mobile releases", "An on-call rota for the data platform", "A game day for payments failover", "An alert review for search"]:
    d.add_paragraph(a, style="List Number")
p = d.sections[0].footer.paragraphs[0]
run = p.add_run()
for kind, text in [("begin", None), (None, " PAGE "), ("end", None)]:
    if kind:
        f = OxmlElement("w:fldChar"); f.set(qn("w:fldCharType"), kind); run._r.append(f)
    else:
        i = OxmlElement("w:instrText"); i.text = text; run._r.append(i)
d.save("incident-report.docx")
PY
