python3 - <<'PY'
from pptx import Presentation
from pptx.chart.data import CategoryChartData
from pptx.enum.chart import XL_CHART_TYPE
from pptx.util import Inches, Pt

p = Presentation()
p.slide_width, p.slide_height = Inches(13.333), Inches(7.5)
s = p.slides.add_slide(p.slide_layouts[0])
s.shapes.title.text = "Q3 review"
s.placeholders[1].text = "Leadership meeting"


def bullets(title, items):
    s = p.slides.add_slide(p.slide_layouts[5])
    s.shapes.title.text = title
    box = s.shapes.add_textbox(Inches(0.8), Inches(1.8), Inches(11.5), Inches(5))
    tf = box.text_frame
    tf.word_wrap = True
    for i, t in enumerate(items):
        para = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
        para.text = t
        para.font.size = Pt(22)
    return s


s = p.slides.add_slide(p.slide_layouts[5])
s.shapes.title.text = "Revenue: 1,265k EUR, up 9%"
data = CategoryChartData()
data.categories = ["July", "August", "September"]
data.add_series("Revenue (EUR k)", (412, 398, 455))
s.shapes.add_chart(XL_CHART_TYPE.COLUMN_CLUSTERED, Inches(1), Inches(1.8), Inches(11), Inches(5.2), data)
bullets("Customers", ["1,840 paying customers (+120)", "Churn 1.9% a month, from 2.4%", "Net revenue retention 108%"])
bullets("Product", ["Shipped: offline mode, audit log, SSO", "Slipped: the reporting API", "Tickets per customer down 14%"])
bullets("Team", ["Headcount 64 (+5)", "Three roles open", "One leaver"])
bullets("Decisions needed", ["Approve the Q4 hiring plan", "Team plan price rise: January or April", "Fund or drop the reporting API rebuild"])
p.save("q3-review.pptx")
PY
