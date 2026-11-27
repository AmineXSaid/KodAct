python3 - <<'PY'
import copy
from pptx import Presentation

p = Presentation("deck.pptx")
p.slides[2].shapes.title.text = "Hiring plan"
s = p.slides.add_slide(p.slides[1].slide_layout)
s.shapes.title.text = "Risks"
tf = s.placeholders[1].text_frame
tf.text = "Schedule: the API slips again"
tf.add_paragraph().text = "Budget: hiring costs above plan"
tf.add_paragraph().text = "Staffing: open roles stay open"
ids = p.slides._sldIdLst
el = ids[-1]
ids.remove(el)
ids.insert(2, el)
p.save("deck.pptx")
PY
