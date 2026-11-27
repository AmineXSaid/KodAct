"""q3-review.pptx: six slides from notes.md, a native chart of the monthly
revenue, the decisions on the last slide, and nothing that renders badly
(text off the slide, text over text, type too small to read)."""

import os
import subprocess
import sys
import tempfile

from pptx import Presentation
from pptx.util import Pt

if not os.path.exists("q3-review.pptx"):
    sys.exit("q3-review.pptx was not made")
try:
    deck = Presentation("q3-review.pptx")
except Exception as e:
    sys.exit(f"q3-review.pptx does not open: {e}")

slides = list(deck.slides)
if len(slides) != 6:
    sys.exit(f"want 6 slides (a title, then one per section), got {len(slides)}")


def text(slide):
    out = []
    for sh in slide.shapes:
        if sh.has_text_frame:
            out.append(sh.text_frame.text)
        if getattr(sh, "has_table", False) and sh.has_table:
            out += [c.text for r in sh.table.rows for c in r.cells]
    return " ".join(out).lower()


# The revenue as a chart PowerPoint can edit, not a picture of one.
charts = [sh.chart for s in slides for sh in s.shapes if getattr(sh, "has_chart", False) and sh.has_chart]
if not charts:
    sys.exit("no native chart in the deck (a picture of a chart can't be edited)")
values = sorted(round(v) for c in charts for p in c.plots for s in p.series for v in s.values if v is not None)
if not all(v in values for v in (412, 398, 455)):
    sys.exit(f"the chart doesn't plot July-September revenue (412, 398, 455): {values}")

last = text(slides[-1])
for word in ("hiring", "price", "reporting"):
    if word not in last:
        sys.exit(f"the last slide should list the three decisions; '{word}' is missing")

W, H = deck.slide_width, deck.slide_height
for n, s in enumerate(slides, 1):
    for sh in s.shapes:
        if sh.left is None or sh.width is None:
            continue
        if sh.left < -Pt(2) or sh.top < -Pt(2) or sh.left + sh.width > W + Pt(2) or sh.top + sh.height > H + Pt(2):
            sys.exit(f"slide {n}: '{sh.name}' runs off the slide")
        if sh.has_text_frame:
            for p in sh.text_frame.paragraphs:
                for r in p.runs:
                    if r.font.size is not None and r.font.size < Pt(10) and r.text.strip():
                        sys.exit(f"slide {n}: text at {r.font.size.pt:g}pt is too small to read: {r.text[:40]!r}")

# Rendered: every slide drawn, no text past the slide's edge, no text on text.
with tempfile.TemporaryDirectory() as tmp:
    out = subprocess.run(
        ["soffice", f"-env:UserInstallation=file://{tmp}/profile", "--headless", "--convert-to", "pdf", "--outdir", tmp, "q3-review.pptx"],
        capture_output=True, text=True, timeout=180,
    )
    pdf = os.path.join(tmp, "q3-review.pdf")
    if not os.path.exists(pdf):
        sys.exit(f"the deck does not render: {out.stderr[-300:]}")
    import pdfplumber

    with pdfplumber.open(pdf) as doc:
        if len(doc.pages) != 6:
            sys.exit(f"rendered {len(doc.pages)} pages, want 6")
        for n, page in enumerate(doc.pages, 1):
            words = page.extract_words(use_text_flow=False, keep_blank_chars=False)
            for w in words:
                if w["x1"] > page.width + 1 or w["bottom"] > page.height + 1 or w["x0"] < -1 or w["top"] < -1:
                    sys.exit(f"slide {n}: '{w['text']}' is drawn past the slide's edge")
            for i, a in enumerate(words):
                for b in words[i + 1:]:
                    ix = min(a["x1"], b["x1"]) - max(a["x0"], b["x0"])
                    iy = min(a["bottom"], b["bottom"]) - max(a["top"], b["top"])
                    if ix <= 0 or iy <= 0:
                        continue
                    small = min((a["x1"] - a["x0"]) * (a["bottom"] - a["top"]), (b["x1"] - b["x0"]) * (b["bottom"] - b["top"]))
                    if small > 0 and ix * iy > 0.3 * small:
                        sys.exit(f"slide {n}: text collides: '{a['text']}' over '{b['text']}'")
print("ok")
