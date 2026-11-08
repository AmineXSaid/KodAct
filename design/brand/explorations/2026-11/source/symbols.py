"""KodAct identity exploration, November 2026: first drafts of ten symbols.

Work in progress: these are the first-pass drawings, not a chosen logo.
Run `python3 symbols.py sheet.html` to write a contact sheet.

Every symbol is drawn on a 64-unit square with an 8-unit margin and returned
as SVG markup in `currentColor`, so one drawing serves black on white and
white on black.
"""
import math

INK = "currentColor"


def poly(pts):
    return "M" + " L".join(f"{x:.2f} {y:.2f}" for x, y in pts) + " Z"


def tangent_hull(c1, r1, c2, r2):
    """Outer tangent points of two circles (counter-clockwise outline)."""
    (x1, y1), (x2, y2) = c1, c2
    dx, dy = x2 - x1, y2 - y1
    d = math.hypot(dx, dy)
    a = math.atan2(dy, dx)
    b = math.acos((r1 - r2) / d)
    t1 = (x1 + r1 * math.cos(a + b), y1 + r1 * math.sin(a + b))
    t2 = (x2 + r2 * math.cos(a + b), y2 + r2 * math.sin(a + b))
    t3 = (x2 + r2 * math.cos(a - b), y2 + r2 * math.sin(a - b))
    t4 = (x1 + r1 * math.cos(a - b), y1 + r1 * math.sin(a - b))
    return t1, t2, t3, t4


# 1. Indent ---------------------------------------------------------------
def indent_svg():
    d = (
        "M8 8 A48 48 0 0 1 56 56 H34 V43 L45.5 43 L45.5 40 L21 40 V27"
        " L33 27 L33 24 L8 24 Z"
    )
    return f'<path fill="{INK}" d="{d}"/>'


# 2. Cam --------------------------------------------------------------------
def cam_svg():
    c1, r1 = (27, 39), 17
    c2, r2 = (43, 23), 6.5
    t1, t2, t3, t4 = tangent_hull(c1, r1, c2, r2)
    d = (
        f"M{t1[0]:.2f} {t1[1]:.2f} L{t2[0]:.2f} {t2[1]:.2f}"
        f" A{r2} {r2} 0 1 0 {t3[0]:.2f} {t3[1]:.2f}"
        f" L{t4[0]:.2f} {t4[1]:.2f}"
        f" A{r1} {r1} 0 1 0 {t1[0]:.2f} {t1[1]:.2f} Z"
        f" M{c1[0]+5} {c1[1]} a5 5 0 1 0 -10 0 a5 5 0 1 0 10 0 Z"
    )
    top = c2[1] - r2
    rod = f'<rect fill="{INK}" x="{c2[0]-4}" y="4" width="8" height="{top-4-3:.2f}"/>'
    return f'<path fill="{INK}" fill-rule="evenodd" d="{d}"/>' + rod


# 3. Heron -----------------------------------------------------------------
def heron_svg():
    neck = (
        f'<path fill="none" stroke="{INK}" stroke-width="9" stroke-linecap="butt"'
        ' d="M20 60 V52 A13 13 0 0 1 33 39 A11.5 11.5 0 0 0 33 16 H30"/>'
    )
    head = f'<circle fill="{INK}" cx="29" cy="20.5" r="9"/>'
    beak = f'<path fill="{INK}" d="M33 13.6 L60 22 L33 27.4 Z"/>'
    eye = '<circle fill="var(--ground,#fff)" cx="30.5" cy="19.5" r="2"/>'
    return neck + head + beak + eye


# 4. Tumbler ---------------------------------------------------------------
def tumbler_svg():
    # Four pin stacks of different lengths (the code); every split lands on
    # one shear line, so the cylinder can turn.
    shear = 34
    gap = 3
    xs = [9, 21, 33, 45]
    tops = [10, 20, 6, 15]
    bottoms = [48, 56, 44, 52]
    out = []
    for x, t, b in zip(xs, tops, bottoms):
        out.append(f'<rect fill="{INK}" x="{x}" y="{t}" width="9" height="{shear-gap/2-t}"/>')
        out.append(f'<rect fill="{INK}" x="{x}" y="{shear+gap/2}" width="9" height="{b-shear-gap/2}"/>')
    return "".join(out)


# 5. Pawl ------------------------------------------------------------------
def pawl_svg():
    teeth = []
    x0, x1, base, h, p = 6, 58, 56, 9, 10.4
    pts = [(x0, base)]
    x = x0
    pts.append((x0, base - 14))
    while x + p <= x1 + 0.1:
        pts.append((x + p, base - 14 - h))
        pts.append((x + p, base - 14))
        x += p
    pts[-1] = (x1, base - 14)
    pts.append((x1, base))
    rack = f'<path fill="{INK}" d="{poly(pts)}"/>'
    # pawl: pivot ring and a tapered arm dropping into one valley
    vx = x0 + 3 * p  # valley x (vertical face of tooth 3)
    vy = base - 14
    piv = (vx - 13, 12)
    arm = poly([(piv[0] - 4.5, piv[1] + 2), (piv[0] + 4.5, piv[1] - 2),
                (vx + 0.5, vy - 3.2), (vx - 4.2, vy - 6.5)])
    pawl = (f'<path fill="{INK}" d="{arm}"/>'
            f'<circle fill="{INK}" cx="{piv[0]}" cy="{piv[1]}" r="6.5"/>'
            f'<circle fill="var(--ground,#fff)" cx="{piv[0]}" cy="{piv[1]}" r="2.6"/>')
    return rack + pawl


# 6. Hinge -----------------------------------------------------------------
def hinge_svg():
    out = [f'<rect fill="{INK}" x="8" y="10" width="19" height="44"/>']
    # right leaf, turned away from us: foreshortened
    out.append(f'<rect fill="{INK}" x="39" y="10" width="13" height="44"/>')
    n, top, bot, g = 5, 10, 54, 2
    seg = (bot - top - g * (n - 1)) / n
    for i in range(n):
        y = top + i * (seg + g)
        if i % 2 == 0:  # belongs to left leaf
            out.append(f'<rect fill="{INK}" x="27" y="{y:.2f}" width="9" height="{seg:.2f}"/>')
        else:
            out.append(f'<rect fill="{INK}" x="29" y="{y:.2f}" width="10" height="{seg:.2f}"/>')
    return "".join(out)


# 7. Half-turn -------------------------------------------------------------
def halfturn_svg():
    # A ribbon given one half-turn: the face you read becomes the face that acts.
    left = (f'<path fill="{INK}" d="M8 14 C20 14 27 24 32 32 C27 40 20 50 8 50 Z"/>')
    right = (f'<path fill="none" stroke="{INK}" stroke-width="3.2" stroke-linejoin="miter"'
             ' d="M54.4 15.6 C44 15.6 37.6 24.6 33.9 32 C37.6 39.4 44 48.4 54.4 48.4 Z"/>')
    return left + right


# 8. Dovetail --------------------------------------------------------------
def dovetail_svg():
    a = poly([(8, 10), (30, 10), (30, 21), (38, 17), (38, 47), (30, 43), (30, 54), (8, 54)])
    b = poly([(33, 10), (56, 10), (56, 54), (33, 54), (33, 45.4), (41, 49.4),
              (41, 14.6), (33, 18.6)])
    return f'<path fill="{INK}" d="{a}"/><path fill="{INK}" d="{b}"/>'


# 9. First tile --------------------------------------------------------------
def firsttile_svg():
    out = []
    for x in (30, 44):
        out.append(f'<rect fill="{INK}" x="{x}" y="14" width="9" height="42"/>')
    # first tile, released, leaning onto the second
    out.append(f'<rect fill="{INK}" x="0" y="0" width="9" height="42"'
               ' transform="translate(11.5 58) rotate(-28) translate(0 -42)"/>')
    return "".join(out)


# 10. Ejected line -------------------------------------------------------------
def ejected_svg():
    plate = ("M8 8 H40 V56 H8 Z M14 17 H34 V23 H14 Z M14 29 H34 V35 H14 Z"
             " M14 41 H28 V47 H14 Z")
    slug = f'<rect fill="{INK}" x="44" y="29" width="20" height="6"/>'
    return f'<path fill="{INK}" fill-rule="evenodd" d="{plate}"/>' + slug


SYMBOLS = [
    ("indent", "Indent", indent_svg),
    ("cam", "Cam", cam_svg),
    ("heron", "Heron", heron_svg),
    ("tumbler", "Tumbler", tumbler_svg),
    ("pawl", "Pawl", pawl_svg),
    ("hinge", "Hinge", hinge_svg),
    ("halfturn", "Half-turn", halfturn_svg),
    ("dovetail", "Dovetail", dovetail_svg),
    ("firsttile", "First tile", firsttile_svg),
    ("ejected", "Ejected line", ejected_svg),
]


def svg(inner, size=64, color="#0b0c0e", ground="#ffffff"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="{size}" height="{size}"'
            f' style="color:{color};--ground:{ground}">{inner}</svg>')


if __name__ == "__main__":
    import sys
    cells = []
    for key, name, fn in SYMBOLS:
        inner = fn()
        cells.append(
            f'<div class="c"><div>{svg(inner,256)}</div><div class="s">{svg(inner,24)}{svg(inner,16)}'
            f'<span style="background:#000;display:inline-flex;padding:4px">{svg(inner,24,"#fff","#000")}</span></div><p>{name}</p></div>')
    html = ("<html><body style='margin:0;font:14px sans-serif'><style>.c{display:inline-block;margin:12px;"
            "border:1px solid #ddd;padding:8px}.s{display:flex;gap:10px;align-items:center}</style>"
            + "".join(cells) + "</body></html>")
    open(sys.argv[1], "w").write(html)
