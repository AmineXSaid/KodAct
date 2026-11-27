"""Make the starting files of the doc-* tasks (run once; the files are committed).

    python3 evals/tasks/doc-fixtures.py

Deterministic: the same files every run, so a task's answers stay fixed.
"""

import csv
import json
import os
import random

HERE = os.path.dirname(os.path.abspath(__file__))


def repo(task, *parts):
    path = os.path.join(HERE, task, "repo", *parts)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    return path


def deck_notes():
    text = """# Q3 review: notes for the leadership meeting

## Revenue
Revenue by month (EUR thousands): July 412, August 398, September 455.
The quarter closed at 1,265, up 9% on Q2. September was the best month of the year,
driven by the two enterprise renewals that slipped from August.

## Customers
We ended the quarter with 1,840 paying customers (+120). Churn fell to 1.9% a month
from 2.4%. Net revenue retention is 108%. The largest new account is Halden Logistics.

## Product
Shipped: offline mode, the audit log, and single sign-on for the Team plan.
Slipped to Q4: the reporting API (blocked on the new permissions model).
Support tickets per customer fell 14% after the onboarding rewrite.

## Team
Headcount is 64 (+5). Two engineering roles and one support role are still open.
Attrition this quarter: one person.

## Decisions needed
1. Approve the Q4 hiring plan (6 roles).
2. Choose between raising Team plan prices in January or in April.
3. Fund the reporting API rebuild or drop it from the roadmap.
"""
    open(repo("doc-deck-from-notes", "notes.md"), "w").write(text)


def edit_deck():
    from pptx import Presentation

    p = Presentation()
    title_only, content = p.slide_layouts[0], p.slide_layouts[1]
    s = p.slides.add_slide(title_only)
    s.shapes.title.text = "Platform roadmap"
    s.placeholders[1].text = "Planning review, October"
    for title, bullets in [
        ("Where we are", ["Offline mode shipped", "Audit log shipped", "SSO on the Team plan"]),
        ("Next quarter", ["Reporting API", "Permissions model", "Faster search"]),
        ("Staffing", ["64 people today", "Three roles open", "One leaver"]),
        ("Asks", ["Approve the plan", "Fund the API rebuild"]),
    ]:
        s = p.slides.add_slide(content)
        s.shapes.title.text = title
        body = s.placeholders[1].text_frame
        body.text = bullets[0]
        for b in bullets[1:]:
            body.add_paragraph().text = b
    p.save(repo("doc-edit-deck", "deck.pptx"))


def incidents():
    teams = [
        ("Payments", 14, 3, 41),
        ("Search", 9, 1, 22),
        ("Mobile", 17, 4, 63),
        ("Identity", 6, 0, 15),
        ("Data platform", 11, 2, 37),
    ]
    with open(repo("doc-report-docx", "incidents.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["team", "incidents", "sev1", "hours_down"])
        w.writerows(teams)
    open(repo("doc-report-docx", "brief.md"), "w").write(
        "# Q3 incident report: brief\n\n"
        "Audience: the engineering leads. Keep it to two pages.\n\n"
        "- Mobile had the most incidents and the most downtime: two bad releases in August.\n"
        "- Identity had no Sev1 this quarter, the first time in a year.\n"
        "- Actions agreed: staged rollouts for mobile releases; an on-call rota for the data\n"
        "  platform; a game day for payments failover; alert review for search.\n"
    )


def expenses():
    rnd = random.Random(7)
    cats = ["Travel", "Software", "Hardware", "Training", "Events", "Office"]
    rows = []
    for i in range(240):
        month = 7 + i % 3
        rows.append(
            [
                f"2026-{month:02d}-{1 + rnd.randrange(28):02d}",
                rnd.choice(cats),
                f"Item {i + 1}",
                round(rnd.uniform(20, 2400), 2),
            ]
        )
    with open(repo("doc-budget-xlsx", "expenses.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["date", "category", "description", "amount"])
        w.writerows(rows)


def pdf_form():
    from reportlab.lib.pagesizes import A4
    from reportlab.pdfgen import canvas

    path = repo("doc-pdf-form", "application.pdf")
    c = canvas.Canvas(path, pagesize=A4)
    c.setFont("Helvetica-Bold", 16)
    c.drawString(72, 780, "Grant application")
    c.setFont("Helvetica", 11)
    form = c.acroForm
    for i, (name, label) in enumerate(
        [("applicant", "Applicant"), ("organisation", "Organisation"), ("date", "Date"), ("amount", "Amount (EUR)")]
    ):
        y = 720 - i * 50
        c.drawString(72, y + 6, label)
        form.textfield(name=name, x=200, y=y, width=300, height=22, borderStyle="inset")
    c.drawString(72, 520, "Matched funding confirmed")
    form.checkbox(name="matched", x=260, y=514, size=18, buttonStyle="check")
    c.showPage()
    c.drawString(72, 780, "Declaration")
    c.drawString(72, 750, "Signed on behalf of the organisation named on page 1.")
    form.textfield(name="signatory", x=200, y=700, width=300, height=22, borderStyle="inset")
    c.drawString(72, 706, "Signatory")
    c.save()

    path = repo("doc-pdf-form", "appendix.pdf")
    c = canvas.Canvas(path, pagesize=A4)
    for n in range(1, 4):
        c.setFont("Helvetica", 12)
        c.drawString(72, 780, f"Appendix: budget detail, page {n}")
        c.showPage()
    c.save()
    json.dump(
        {
            "applicant": "Amira Haddad",
            "organisation": "North Valley Science Club",
            "date": "2026-11-02",
            "amount": "18500",
            "matched_funding": True,
            "signatory": "Amira Haddad",
        },
        open(repo("doc-pdf-form", "details.json"), "w"),
        indent=2,
    )


def sales():
    rnd = random.Random(11)
    regions = ["North", "South", "East", "West"]
    products = ["Basic", "Plus", "Pro", "Enterprise"]
    price = {"Basic": 9, "Plus": 19, "Pro": 49, "Enterprise": 199}
    with open(repo("doc-big-workbook", "sales.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["date", "region", "product", "units", "revenue"])
        for _ in range(50000):
            month = 1 + rnd.randrange(12)
            p = rnd.choice(products)
            units = 1 + rnd.randrange(20)
            w.writerow(
                [f"2026-{month:02d}-{1 + rnd.randrange(28):02d}", rnd.choice(regions), p, units, units * price[p]]
            )


if __name__ == "__main__":
    deck_notes()
    edit_deck()
    incidents()
    expenses()
    pdf_form()
    sales()
    print("made")
