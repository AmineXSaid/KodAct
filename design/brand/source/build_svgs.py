"""Builds KodAct's logo files (SVG, PNG, ICO) from one set of drawings.

The mark is two strokes on one slope: a step up from the left, then a
chevron pointing right. Code goes in, impact comes out. Every slanted edge
shares the same angle, so the mark reads as one gesture at any size.

The name is set in Outfit (SemiBold for "KodAct", Medium tracked wide for the
tagline) and converted to outlines, so no file depends on an installed font.

    pip install fonttools uharfbuzz cairosvg pillow
    # Outfit[wght].ttf from github.com/google/fonts (ofl/outfit), saved as outfit.ttf
    python3 source/build_svgs.py <output folder> <font folder>
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
os.makedirs(os.path.join(OUT, "png"), exist_ok=True)

# The palette (the terminal's is in crates/kodact-cli/src/tui/palette.rs).
BLUE = "#0a5cff"  # Signal blue: the mark and "Act" on light backgrounds, the app icon's tile
BLUE_LIFT = "#4d8dff"  # Signal blue lifted for dark backgrounds
INK = "#0b0c0e"  # "Kod" on light backgrounds, the dark tile
PAPER = "#ffffff"  # "Kod" on dark backgrounds, the mark on the blue tile
SLATE = "#5a5f69"  # The tagline on light backgrounds
MIST = "#9a9fa8"  # The tagline on dark backgrounds

# ---------- the mark ----------

# Drawn on the trace's grid; every slanted edge has the slope S (dy/dx), about 39 degrees.
S = 0.82


def _mark_polys():
    a = [
        (123, 462),
        (123 + 142 / S, 320),
        (360, 320),
        (360 + 45 / S, 365),
        (363, 365),
        (244.7, 462),
    ]
    b = [
        (435, 224),
        (435 + 149 / S, 373),
        (435 + 149 / S - 145 / S, 518),
        (290, 518),
        (290 + 112 / S, 406),
        (466, 406),
        (466 + 26 / S, 380),
        (466 + 26 / S - 83 / S, 297),
        (435 - 73 / S, 297),
    ]
    return a, b


MARK_BOX = (119, 220, 616.7 - 119 + 4 + 4, 518 - 220 + 4)  # x, y, w, h with the rounding


def mark(fill, scale=1.0, dx=0.0, dy=0.0):
    """The mark, its corners softened by a stroke of its own colour."""
    def path(poly):
        pts = [(dx + (x - MARK_BOX[0]) * scale, dy + (y - MARK_BOX[1]) * scale) for x, y in poly]
        return "M" + "L".join(f"{x:.2f} {y:.2f}" for x, y in pts) + "Z"

    a, b = _mark_polys()
    sw = 8 * scale
    return (f'<path fill="{fill}" stroke="{fill}" stroke-width="{sw:.2f}" stroke-linejoin="round" '
            f'd="{path(a)}{path(b)}"/>')


# ---------- the name, as outlines ----------

def static_font(path, axes):
    tt = TTFont(path)
    inst = instancer.instantiateVariableFont(tt, axes)
    buf = io.BytesIO()
    inst.save(buf)
    data = buf.getvalue()
    return TTFont(io.BytesIO(data)), data


def set_text(path, axes, text, size, x, baseline, tracking=0.0):
    """Shapes `text` (with kerning); returns (svg path d, advance, bounds)."""
    tt, data = static_font(path, axes)
    upem = tt["head"].unitsPerEm
    font = hb.Font(hb.Face(data))
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, {"kern": True})
    gs = tt.getGlyphSet()
    order = tt.getGlyphOrder()
    scale = size / upem
    pen_x = 0.0
    d = []
    box = [float("inf"), float("inf"), float("-inf"), float("-inf")]
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        name = order[info.codepoint]
        m = (scale, 0, 0, -scale, x + (pen_x + pos.x_offset) * scale, baseline - pos.y_offset * scale)
        pen = SVGPathPen(gs)
        gs[name].draw(TransformPen(pen, m))
        bp = BoundsPen(gs)
        gs[name].draw(TransformPen(bp, m))
        if bp.bounds:
            x0, y0, x1, y1 = bp.bounds
            box = [min(box[0], x0), min(box[1], y0), max(box[2], x1), max(box[3], y1)]
        d.append(pen.getCommands())
        pen_x += pos.x_advance + tracking * upem
    return " ".join(d), (pen_x - tracking * upem) * scale, box


OUTFIT = os.path.join(FONTS, "outfit.ttf")
NAME_AXES = {"wght": 620}
TAG_AXES = {"wght": 500}


def cap_height(size):
    tt, _ = static_font(OUTFIT, NAME_AXES)
    return tt["OS/2"].sCapHeight / tt["head"].unitsPerEm * size


def name(x, baseline, size, kod_fill, act_fill):
    """'Kod' then 'Act', one word in two colours: (svg, width, bounds)."""
    kod_d, kod_w, kb = set_text(OUTFIT, NAME_AXES, "Kod", size, x, baseline, tracking=-0.015)
    act_x = x + kod_w - 0.015 * size
    act_d, act_w, ab = set_text(OUTFIT, NAME_AXES, "Act", size, act_x, baseline, tracking=-0.015)
    svg = f'<path fill="{kod_fill}" d="{kod_d}"/><path fill="{act_fill}" d="{act_d}"/>'
    box = (min(kb[0], ab[0]), min(kb[1], ab[1]), max(kb[2], ab[2]), max(kb[3], ab[3]))
    return svg, act_x + act_w - x, box


def tagline(x, baseline, size, fill, width=None):
    """FROM CODE TO IMPACT, tracked wide; stretched by tracking to `width` if given."""
    text = "FROM CODE TO IMPACT"
    track = 0.26
    _, w, _ = set_text(OUTFIT, TAG_AXES, text, size, 0, 0, tracking=track)
    if width:
        track += (width - w) / (size * (len(text) - 1))
    d, w, box = set_text(OUTFIT, TAG_AXES, text, size, x, baseline, tracking=track)
    return f'<path fill="{fill}" d="{d}"/>', w, box


# ---------- documents ----------

def svg_doc(view, body, title):
    x, y, vw, vh = view
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:g} {y:g} {vw:.2f} {vh:.2f}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def write(fname, text):
    with open(os.path.join(OUT, fname), "w", encoding="utf-8") as f:
        f.write(text)


def colours(on_dark):
    return (BLUE_LIFT, PAPER, BLUE_LIFT, MIST) if on_dark else (BLUE, INK, BLUE, SLATE)


# The mark alone, in colour and in one colour.
mw, mh = MARK_BOX[2], MARK_BOX[3]
for fname, fill, title in [
    ("kodact-mark.svg", BLUE, "KodAct mark"),
    ("kodact-mark-on-dark.svg", BLUE_LIFT, "KodAct mark, for dark backgrounds"),
    ("kodact-mark-ink.svg", INK, "KodAct mark, one colour"),
    ("kodact-mark-paper.svg", PAPER, "KodAct mark, one colour for dark backgrounds"),
]:
    write(fname, svg_doc((0, 0, mw, mh), mark(fill), title))


# The app icon: the mark on a tile, 256 grid.
def app_icon(tile, fill):
    s = 172 / mw  # the mark spans 172 of the tile's 240, so it holds up at 16 px
    return (f'<rect fill="{tile}" x="8" y="8" width="240" height="240" rx="56"/>'
            + mark(fill, s, 128 - mw * s / 2, 128 - mh * s / 2 + 4))


write("app-icon.svg", svg_doc((0, 0, 256, 256), app_icon(BLUE, PAPER), "KodAct app icon"))
write("app-icon-dark.svg", svg_doc((0, 0, 256, 256), app_icon(INK, BLUE), "KodAct app icon, dark"))


def lockup(on_dark, stacked=False):
    mark_fill, kod_fill, act_fill, tag_fill = colours(on_dark)
    size = 100
    cap = cap_height(size)
    if not stacked:
        # The mark a little shorter than the capitals and tagline together, centred on them.
        _, nw, _ = name(0, 0, size, kod_fill, act_fill)
        tag_size = size * 0.17
        gap_tag = size * 0.2
        block_h = cap + gap_tag + tag_size * 0.72
        s = block_h * 0.86 / mh
        mx, my = 0, 0
        tx = mw * s + size * 0.26
        base = my + (mh * s - block_h) / 2 + cap
        text, nw, box = name(tx, base, size, kod_fill, act_fill)
        tag, _, tbox = tagline(tx + size * 0.01, base + gap_tag + tag_size * 0.72, tag_size, tag_fill, width=nw - size * 0.03)
        right = max(box[2], tbox[2])
        pad = size * 0.1
        view = (-pad, min(box[1], my) - pad, right + 2 * pad, max(tbox[3], mh * s) - min(box[1], my) + 2 * pad)
        return svg_doc(view, mark(mark_fill, s, mx, my) + text + tag, "KodAct, from code to impact")
    # Stacked: the mark over the name, the tagline under it.
    _, nw, _ = name(0, 0, size, kod_fill, act_fill)
    s = nw * 0.5 / mw
    mx = (nw - mw * s) / 2
    base = mh * s + size * 0.3 + cap
    text, nw, box = name(0, base, size, kod_fill, act_fill)
    tag_size = size * 0.17
    tag, _, tbox = tagline(size * 0.01, base + size * 0.2 + tag_size * 0.72, tag_size, tag_fill, width=nw - size * 0.03)
    pad = size * 0.12
    view = (-pad, -pad, nw + 2 * pad, tbox[3] + 2 * pad)
    return svg_doc(view, mark(mark_fill, s, mx, 0) + text + tag, "KodAct, from code to impact")


def wordmark_only(on_dark):
    _, kod_fill, act_fill, _ = colours(on_dark)
    text, _, (x0, y0, x1, y1) = name(0, 100, 100, kod_fill, act_fill)
    pad = 8
    return svg_doc((x0 - pad, y0 - pad, x1 - x0 + 2 * pad, y1 - y0 + 2 * pad), text, "KodAct")


write("kodact-logo.svg", lockup(False))
write("kodact-logo-on-dark.svg", lockup(True))
write("kodact-logo-stacked.svg", lockup(False, stacked=True))
write("kodact-logo-stacked-on-dark.svg", lockup(True, stacked=True))
write("kodact-wordmark.svg", wordmark_only(False))
write("kodact-wordmark-on-dark.svg", wordmark_only(True))

# ---------- PNG and ICO ----------

import cairosvg  # noqa: E402
from PIL import Image  # noqa: E402


def png(src, dst, width):
    cairosvg.svg2png(url=os.path.join(OUT, src), write_to=os.path.join(OUT, "png", dst), output_width=width)


for n in [16, 24, 32, 48, 64, 128, 256, 512, 1024]:
    png("app-icon.svg", f"app-icon-{n}.png", n)
for n in [256, 512, 1024]:
    png("app-icon-dark.svg", f"app-icon-dark-{n}.png", n)
for n in [256, 512]:
    png("kodact-mark.svg", f"kodact-mark-{n}.png", n)
    png("kodact-mark-on-dark.svg", f"kodact-mark-on-dark-{n}.png", n)
for stem in ["kodact-logo", "kodact-logo-on-dark", "kodact-logo-stacked", "kodact-logo-stacked-on-dark",
             "kodact-wordmark", "kodact-wordmark-on-dark"]:
    png(f"{stem}.svg", f"{stem}.png", 1200)


def ico(dst, sizes):
    imgs = [Image.open(os.path.join(OUT, "png", f"app-icon-{n}.png")).convert("RGBA") for n in sizes]
    imgs[-1].save(os.path.join(OUT, dst), sizes=[(n, n) for n in sizes], append_images=imgs[:-1])


ico("kodact.ico", [16, 24, 32, 48, 64, 128, 256])
ico("favicon.ico", [16, 32, 48])
print("wrote", sorted(os.listdir(OUT)))
