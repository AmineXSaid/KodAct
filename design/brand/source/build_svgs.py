"""Builds KodAct's logo files (SVG) from one set of drawings.

Kod, the character, is drawn on a 200-unit grid; the one-colour mark on 64;
the app icon on 256 (and a pixel-fitted 16x16). The name is set in
JetBrains Mono (Kod) and Archivo (Act) and converted to outlines, so no file
depends on an installed font.
"""
import io
import os
import sys

import uharfbuzz as hb
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

OUT = sys.argv[1]
FONTS = sys.argv[2]
os.makedirs(OUT, exist_ok=True)

INK = "#101214"
PAPER = "#f2f0e9"
LIME = "#c6ff3d"
OLIVE = "#4a7200"

# ---------- the name, as outlines ----------

def static_font(path, axes):
    tt = TTFont(path)
    inst = instancer.instantiateVariableFont(tt, axes)
    buf = io.BytesIO()
    inst.save(buf)
    data = buf.getvalue()
    return TTFont(io.BytesIO(data)), data


def set_text(path, axes, text, size, x, baseline, tracking=0.0):
    """Shapes `text` (with kerning) and returns (svg path d, advance) in user units."""
    tt, data = static_font(path, axes)
    upem = tt["head"].unitsPerEm
    face = hb.Face(data)
    font = hb.Font(face)
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, {"kern": True, "liga": False})
    gs = tt.getGlyphSet()
    order = tt.getGlyphOrder()
    scale = size / upem
    pen_x = 0.0
    d = []
    box = [float("inf"), float("inf"), float("-inf"), float("-inf")]
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        name = order[info.codepoint]
        pen = SVGPathPen(gs)
        # font units (y up) to SVG (y down), placed on the baseline
        m = (scale, 0, 0, -scale, x + (pen_x + pos.x_offset) * scale, baseline - pos.y_offset * scale)
        gs[name].draw(TransformPen(pen, m))
        bp = BoundsPen(gs)
        gs[name].draw(TransformPen(bp, m))
        if bp.bounds:
            x0, y0, x1, y1 = bp.bounds
            box = [min(box[0], x0), min(box[1], y0), max(box[2], x1), max(box[3], y1)]
        d.append(pen.getCommands())
        pen_x += pos.x_advance + tracking * upem
    set_text.last_box = box
    return " ".join(d), (pen_x - tracking * upem) * scale, tt


JB = os.path.join(FONTS, "jbmono.ttf")
AR = os.path.join(FONTS, "archivo.ttf")
JB_AXES = {"wght": 500}
AR_AXES = {"wght": 850, "wdth": 114}


def wordmark(x, baseline, size, kod_fill, act_fill):
    """'Kod' in JetBrains Mono, 'Act' in Archivo: returns (svg, width, cap height)."""
    kod_d, kod_w, _ = set_text(JB, JB_AXES, "Kod", size, x, baseline, tracking=-0.04)
    box_kod = set_text.last_box
    gap = size * 0.02
    act_d, act_w, ar = set_text(AR, AR_AXES, "Act", size, x + kod_w + gap, baseline, tracking=-0.01)
    box_act = set_text.last_box
    cap = ar["OS/2"].sCapHeight / ar["head"].unitsPerEm * size
    svg = f'<path fill="{kod_fill}" d="{kod_d}"/><path fill="{act_fill}" d="{act_d}"/>'
    wordmark.box = (min(box_kod[0], box_act[0]), min(box_kod[1], box_act[1]), max(box_kod[2], box_act[2]), max(box_kod[3], box_act[3]))
    return svg, kod_w + gap + act_w, cap


# ---------- Kod, the character (200 grid) ----------

BRACE_L = "M48 60Q34 60 35 75L36 88Q36 97 26 98Q36 99 36 108L35 121Q34 136 48 136"
BRACE_R = "M152 60Q166 60 165 75L164 88Q164 97 174 98Q164 99 164 108L165 121Q166 136 152 136"
FACE = (f'<ellipse fill="{INK}" cx="85" cy="90" rx="7.5" ry="10"/><ellipse fill="{INK}" cx="115" cy="90" rx="7.5" ry="10"/>'
        f'<circle fill="{PAPER}" cx="87.5" cy="85.5" r="2.8"/><circle fill="{PAPER}" cx="117.5" cy="85.5" r="2.8"/>'
        f'<rect fill="{INK}" x="93" y="108" width="14" height="5.5" rx="1.5"/>')


def kod_art(on_dark=False):
    if not on_dark:
        return (f'<rect fill="{INK}" x="76" y="138" width="18" height="26" rx="9"/><rect fill="{INK}" x="106" y="138" width="18" height="26" rx="9"/>'
                f'<rect fill="{LIME}" stroke="{INK}" stroke-width="7" x="56" y="44" width="88" height="104" rx="38"/>'
                f'{FACE}'
                f'<path fill="none" stroke="{INK}" stroke-width="9" stroke-linecap="round" stroke-linejoin="round" d="{BRACE_L}{BRACE_R}"/>')
    # On a dark background the ink outline would vanish, so the braces and legs turn lime;
    # a thin ink line keeps the legs apart from the body.
    return (f'<rect fill="{LIME}" x="76" y="138" width="18" height="26" rx="9"/><rect fill="{LIME}" x="106" y="138" width="18" height="26" rx="9"/>'
            f'<rect fill="{LIME}" stroke="{INK}" stroke-width="4" x="54" y="42" width="92" height="108" rx="40"/>'
            f'{FACE}'
            f'<path fill="none" stroke="{LIME}" stroke-width="9" stroke-linecap="round" stroke-linejoin="round" d="{BRACE_L}{BRACE_R}"/>')


KOD_BOX = (16, 36, 168, 132)  # x, y, w, h around the character


def svg_doc(view, body, title, w=None, h=None):
    x, y, vw, vh = view
    size = f' width="{w}" height="{h}"' if w else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:g} {y:g} {vw:g} {vh:g}"{size} role="img" aria-label="{title}">'
            f'<title>{title}</title>{body}</svg>\n')


def write(name, text):
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        f.write(text)


write("kod.svg", svg_doc(KOD_BOX, kod_art(False), "Kod, the KodAct character"))
write("kod-on-dark.svg", svg_doc(KOD_BOX, kod_art(True), "Kod, the KodAct character, for dark backgrounds"))

# ---------- the one-colour mark (64 grid) ----------

def mark(fill):
    return (f'<rect fill="none" stroke="{fill}" stroke-width="4" x="18" y="12.5" width="28" height="33" rx="11.5"/>'
            f'<ellipse fill="{fill}" cx="27" cy="27" rx="3.2" ry="4.2"/><ellipse fill="{fill}" cx="37" cy="27" rx="3.2" ry="4.2"/>'
            f'<rect fill="{fill}" x="29" y="34" width="6" height="2.8" rx=".7"/>'
            f'<rect fill="{fill}" x="23" y="45" width="6.5" height="8.5" rx="3.2"/><rect fill="{fill}" x="34.5" y="45" width="6.5" height="8.5" rx="3.2"/>'
            f'<path fill="none" stroke="{fill}" stroke-width="4.4" stroke-linecap="round" stroke-linejoin="round" '
            f'd="M13 15Q8 15 8.4 20L8.8 25Q8.8 28.5 4.6 29Q8.8 29.5 8.8 33L8.4 38Q8 43 13 43M51 15Q56 15 55.6 20L55.2 25Q55.2 28.5 59.4 29Q55.2 29.5 55.2 33L55.6 38Q56 43 51 43"/>')


write("kod-mark.svg", svg_doc((0, 0, 64, 64), mark(INK), "Kod, one-colour mark"))
write("kod-mark-paper.svg", svg_doc((0, 0, 64, 64), mark(PAPER), "Kod, one-colour mark for dark backgrounds"))

# ---------- the app icon: the lime tile is Kod's body (256 grid) ----------

def app_icon(bold=False):
    # bold: thicker features for 24 and 32 pixels
    sw, ery, erx, mh = (20, 18, 13.5, 14) if bold else (14, 15.5, 11.6, 8.5)
    bl = "M78.1 69.1Q56.4 69.1 57.95 92.35L59.5 112.5Q59.5 126.45 44 128Q59.5 129.55 59.5 143.5L57.95 163.65Q56.4 186.9 78.1 186.9"
    br = "M177.9 69.1Q199.6 69.1 198.05 92.35L196.5 112.5Q196.5 126.45 212 128Q196.5 129.55 196.5 143.5L198.05 163.65Q199.6 186.9 177.9 186.9"
    hl = "" if bold else f'<circle fill="{PAPER}" cx="108.6" cy="108.6" r="4.3"/><circle fill="{PAPER}" cx="155.1" cy="108.6" r="4.3"/>'
    mw = 26 if bold else 21.7
    return (f'<rect fill="{LIME}" x="8" y="8" width="240" height="240" rx="64"/>'
            f'<path fill="none" stroke="{INK}" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round" d="{bl}{br}"/>'
            f'<ellipse fill="{INK}" cx="104.75" cy="115.6" rx="{erx}" ry="{ery}"/><ellipse fill="{INK}" cx="151.25" cy="115.6" rx="{erx}" ry="{ery}"/>{hl}'
            f'<rect fill="{INK}" x="{128 - mw / 2:g}" y="{147.75 - mh / 2:g}" width="{mw:g}" height="{mh:g}" rx="2.5"/>')


write("app-icon.svg", svg_doc((0, 0, 256, 256), app_icon(False), "KodAct app icon"))
write("app-icon-small.svg", svg_doc((0, 0, 256, 256), app_icon(True), "KodAct app icon, tuned for 24 and 32 pixels"))

# 16x16, fitted to the pixel grid by hand: the tile is the body, then braces, eyes and the cursor mouth.
PIX_INK = [(3, 3), (4, 3), (2, 4), (2, 5), (2, 6), (1, 7), (1, 8), (2, 9), (2, 10), (2, 11), (3, 12), (4, 12),
           (12, 3), (11, 3), (13, 4), (13, 5), (13, 6), (14, 7), (14, 8), (13, 9), (13, 10), (13, 11), (12, 12), (11, 12),
           (6, 6), (6, 7), (9, 6), (9, 7), (7, 9), (8, 9)]
corners = {(0, 0), (1, 0), (0, 1), (15, 0), (14, 0), (15, 1), (0, 15), (1, 15), (0, 14), (15, 15), (14, 15), (15, 14)}
tile = "".join(f'<rect x="{x}" y="{y}" width="1" height="1"/>' for y in range(16) for x in range(16) if (x, y) not in corners)
ink = "".join(f'<rect x="{x}" y="{y}" width="1" height="1"/>' for x, y in PIX_INK)
write("app-icon-16.svg", svg_doc((0, 0, 16, 16), f'<g fill="{LIME}" shape-rendering="crispEdges">{tile}</g><g fill="{INK}" shape-rendering="crispEdges">{ink}</g>',
                                 "KodAct app icon, 16 pixels"))

# ---------- the logo: Kod and the name ----------

def lockup(on_dark, stacked=False):
    kod_fill, act_fill = (PAPER, LIME) if on_dark else (INK, OLIVE)
    art = kod_art(on_dark)
    if not stacked:
        size = 92
        x0 = KOD_BOX[0] + KOD_BOX[2] + 18
        text, tw, cap = wordmark(x0, 0, size, kod_fill, act_fill)
        # centre the capitals on Kod's body (y 44..148, middle 96)
        base = 96 + cap / 2
        text, tw, cap = wordmark(x0, base, size, kod_fill, act_fill)
        view = (KOD_BOX[0], KOD_BOX[1], KOD_BOX[2] + 18 + tw + 10, KOD_BOX[3])
        return svg_doc(view, art + text, "KodAct")
    size = 70
    probe, tw, cap = wordmark(0, 0, size, kod_fill, act_fill)
    x0 = 100 - tw / 2
    base = KOD_BOX[1] + KOD_BOX[3] + 22 + cap
    text, tw, cap = wordmark(x0, base, size, kod_fill, act_fill)
    left = min(KOD_BOX[0], x0 - 8)
    right = max(KOD_BOX[0] + KOD_BOX[2], x0 + tw + 8)
    view = (left, KOD_BOX[1], right - left, base + 16 - KOD_BOX[1])
    return svg_doc(view, art + text, "KodAct")


def wordmark_only(on_dark):
    kod_fill, act_fill = (PAPER, LIME) if on_dark else (INK, OLIVE)
    text, tw, cap = wordmark(0, 100, 100, kod_fill, act_fill)
    x0, y0, x1, y1 = wordmark.box
    pad = 8
    return svg_doc((x0 - pad, y0 - pad, x1 - x0 + 2 * pad, y1 - y0 + 2 * pad), text, "KodAct")


write("kodact-logo.svg", lockup(False))
write("kodact-logo-on-dark.svg", lockup(True))
write("kodact-logo-stacked.svg", lockup(False, stacked=True))
write("kodact-logo-stacked-on-dark.svg", lockup(True, stacked=True))
write("kodact-wordmark.svg", wordmark_only(False))
write("kodact-wordmark-on-dark.svg", wordmark_only(True))
print("wrote", sorted(os.listdir(OUT)))
