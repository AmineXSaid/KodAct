"""deck.pptx edited in place: slide 3 retitled, a "Risks" slide added after
slide 2 on slide 2's layout with its bullets in the layout's body, and every
other slide as it was."""

import sys

from pptx import Presentation

try:
    deck = Presentation("deck.pptx")
except Exception as e:
    sys.exit(f"deck.pptx does not open: {e}")
slides = list(deck.slides)
if len(slides) != 6:
    sys.exit(f"want 6 slides (5 and the new one), got {len(slides)}")


def title(s):
    return (s.shapes.title.text if s.shapes.title is not None else "").strip()


def body(s):
    return [sh.text_frame.text for sh in s.placeholders if sh.placeholder_format.idx != 0 and sh.has_text_frame]


titles = [title(s) for s in slides]
want = ["Platform roadmap", "Where we are", "Risks", "Hiring plan", "Staffing", "Asks"]
if titles != want:
    sys.exit(f"want the slides titled {want}, got {titles}")

new = slides[2]
if new.slide_layout.name != slides[1].slide_layout.name:
    sys.exit(f"the Risks slide uses '{new.slide_layout.name}', not slide 2's '{slides[1].slide_layout.name}'")
text = " ".join(body(new)).lower()
if not text:
    sys.exit("the Risks slide's bullets aren't in its layout's body placeholder")
for w in ("schedule", "budget", "staff"):
    if w not in text:
        sys.exit(f"the Risks slide should cover schedule, budget and staffing; '{w}' is missing")
paras = [p for sh in new.placeholders if sh.placeholder_format.idx != 0 and sh.has_text_frame for p in sh.text_frame.paragraphs if p.text.strip()]
if len(paras) < 3:
    sys.exit(f"want three bullets on the Risks slide, got {len(paras)}")

kept = {
    0: ["Planning review, October"],
    1: ["Offline mode shipped\nAudit log shipped\nSSO on the Team plan"],
    3: ["Reporting API\nPermissions model\nFaster search"],
    4: ["64 people today\nThree roles open\nOne leaver"],
    5: ["Approve the plan\nFund the API rebuild"],
}
for i, b in kept.items():
    if body(slides[i]) != b:
        sys.exit(f"slide {i + 1} ({titles[i]}) changed: {body(slides[i])}")
print("ok")
