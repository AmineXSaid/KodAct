"""KodAct identity exploration, November 2026: the symbol drawings.

Every master symbol is drawn on a 64-unit square with an 8-unit margin and
filled with `currentColor`, so one drawing serves black on white and white on
black. The three finalists also have drawings made for 16 and 24 pixel grids,
where the master would blur.

`python3 symbols.py sheet.html` writes a contact sheet of every drawing.
"""

import math

INK = "currentColor"

_uid = [0]


def uid(prefix):
    _uid[0] += 1
    return f"{prefix}{_uid[0]}"


def poly(pts):
    return "M" + " L".join(f"{x:g} {y:g}" for x, y in pts) + " Z"


def tangent_hull(c1, r1, c2, r2):
    """Outer tangent points of two circles."""
    (x1, y1), (x2, y2) = c1, c2
    dx, dy = x2 - x1, y2 - y1
    d = math.hypot(dx, dy)
    a = math.atan2(dy, dx)
    b = math.acos((r1 - r2) / d)
    p = lambda c, r, t: (c[0] + r * math.cos(t), c[1] + r * math.sin(t))
    return p(c1, r1, a + b), p(c2, r2, a + b), p(c2, r2, a - b), p(c1, r1, a - b)


# ---------------------------------------------------------------------------
# Finalist A. Indent: three lines of code, each one step deeper, fused on the
# right into one quarter-turn sweep.
# ---------------------------------------------------------------------------
def _indent(x0, y0, r, rows, steps, gap_ends):
    """x0, y0: top-left corner; r: radius of the sweep.

    rows: [(top, bottom)] for the three lines; steps: left x of each line;
    gap_ends: x where each gap between lines stops and the sweep takes over.
    """
    (t1, b1), (t2, b2), (t3, b3) = rows
    s1, s2, s3 = steps
    g1, g2 = gap_ends
    cx, cy = x0, y0 + r
    d = (
        f"M{s1} {t1} A{r} {r} 0 0 1 {cx + r} {cy} H{s3} V{t3} H{g2} V{b2} H{s2}"
        f" V{t2} H{g1} V{b1} H{s1} Z"
    )
    return f'<path fill="{INK}" d="{d}"/>'


def indent_svg():
    return _indent(8, 8, 48, [(8, 22), (25, 39), (42, 56)], [8, 20, 32], [34, 46])


def indent_24():
    return _indent(3, 3, 18, [(3, 9), (10, 15), (16, 21)], [3, 8, 13], [13, 18])


def indent_16():
    return _indent(2, 2, 12, [(2, 6), (7, 10), (11, 14)], [2, 5, 8], [8, 11])


# ---------------------------------------------------------------------------
# Explored. Cam: a turning lobe lifts a follower. Rotation, the circular
# work of reasoning, becomes one straight, measured stroke.
# ---------------------------------------------------------------------------
def _cam(c1, r1, c2, r2, hole, rod, gap):
    t1, t2, t3, t4 = tangent_hull(c1, r1, c2, r2)
    f = lambda p: f"{p[0]:.2f} {p[1]:.2f}"
    d = (
        f"M{f(t1)} L{f(t2)} A{r2} {r2} 0 0 0 {f(t3)} L{f(t4)}"
        f" A{r1} {r1} 0 1 0 {f(t1)} Z"
        f" M{c1[0] + hole} {c1[1]} a{hole} {hole} 0 1 0 {-2 * hole} 0"
        f" a{hole} {hole} 0 1 0 {2 * hole} 0 Z"
    )
    rx, rw, rtop = rod
    rbot = c2[1] - r2 - gap
    return (
        f'<path fill="{INK}" fill-rule="evenodd" d="{d}"/>'
        f'<rect fill="{INK}" x="{rx}" y="{rtop}" width="{rw}" height="{rbot - rtop:.2f}"/>'
    )


def cam_svg():
    return _cam((26, 38.5), 17.5, (43, 21.5), 7.5, 5.5, (39, 8, 4), 3)


def cam_24():
    return _cam((10, 14.5), 6.5, (16, 8.5), 3, 2, (14.5, 3, 1), 1.2)


def cam_16():
    return _cam((6.5, 9.75), 4.5, (10.75, 5.5), 2, 1.5, (9.75, 2, 0.5), 1)


# ---------------------------------------------------------------------------
# Finalist C. Half-turn: one ribbon given a single half-turn. The face you
# read (outlined: the plan) turns over into the face that acts (solid).
# ---------------------------------------------------------------------------
def _halfturn(x0, x1, y0, y1, w, k=0.5):
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    h = x1 - cx
    a, b = k * h, (1 - k * 0.2) * h  # control-point spread, sets the twist

    def lobe(x_edge, sgn):
        return (
            f"M{x_edge} {y0} C{x_edge - sgn * a:.2f} {y0} {cx + sgn * (h - b):.2f} {cy - (cy - y0) * 0.45:.2f} {cx} {cy}"
            f" C{cx + sgn * (h - b):.2f} {cy + (y1 - cy) * 0.45:.2f} {x_edge - sgn * a:.2f} {y1} {x_edge} {y1} Z"
        )

    plan = lobe(x0, -1)
    act = lobe(x1, 1)
    cid = uid("ht")
    return (
        f'<clipPath id="{cid}"><path d="{plan}"/></clipPath>'
        f'<path fill="none" stroke="{INK}" stroke-width="{2 * w}" clip-path="url(#{cid})" d="{plan}"/>'
        f'<path fill="{INK}" d="{act}"/>'
    )


def halfturn_svg():
    return _halfturn(8, 56, 13, 51, 3.4)


def halfturn_24():
    return _halfturn(2, 22, 5, 19, 1.8)


def halfturn_16():
    return _halfturn(1, 15, 3, 13, 1.5)


# ---------------------------------------------------------------------------
# The other seven concepts (explored, not developed).
# ---------------------------------------------------------------------------
def _heron(k, stroke, eye, dx=-6, dy=-3.5):
    """Heron, drawn on the master grid and scaled by k.

    The neck keeps the heron's kink, the strike held in reserve; the head and
    bill are one shape, the eye a hole through it.
    """
    P = lambda x, y: ((x + dx) * k, (y + dy) * k)
    cx, cy = P(30.5, 20)
    r = 9.5 * k
    px, py = P(62, 21)
    d = math.hypot(px - cx, py - cy)
    base = math.atan2(py - cy, px - cx)
    off = math.acos(r / d)
    t1 = (cx + r * math.cos(base - off), cy + r * math.sin(base - off))
    t2 = (cx + r * math.cos(base + off), cy + r * math.sin(base + off))
    ex, ey = P(32.5, 17.5)
    hole = (
        f" M{ex + eye:.2f} {ey:.2f} a{eye} {eye} 0 1 0 {-2 * eye} 0 a{eye} {eye} 0 1 0 {2 * eye} 0 Z"
        if eye
        else ""
    )
    head = (
        f"M{t2[0]:.2f} {t2[1]:.2f} A{r:.2f} {r:.2f} 0 1 1 {t1[0]:.2f} {t1[1]:.2f}"
        f" L{px:.2f} {py:.2f} Z" + hole
    )
    n0, n1, n2, n3 = P(18.5, 60), P(18.5, 46), P(30.5, 34), P(30.5, 20)
    neck = (
        f'<path fill="none" stroke="{INK}" stroke-width="{stroke}" stroke-linejoin="miter"'
        f' d="M{n0[0]:.2f} {n0[1]:.2f} V{n1[1]:.2f} L{n2[0]:.2f} {n2[1]:.2f} V{n3[1]:.2f}"/>'
    )
    return neck + f'<path fill="{INK}" fill-rule="evenodd" d="{head}"/>'


def heron_svg():
    return _heron(1, 9, 2.2)


def heron_24():
    return _heron(0.375, 3.6, 1.0)


def heron_16():
    return _heron(0.25, 2.8, 0)


def tumbler_svg():
    """Tumbler: pins of different lengths (the code) split on one shear line."""
    shear, gap = 34, 3
    out = []
    for x, t, b in zip([9, 21, 33, 45], [10, 20, 6, 15], [48, 56, 44, 52]):
        out.append(f'<rect fill="{INK}" x="{x}" y="{t}" width="9" height="{shear - gap / 2 - t}"/>')
        out.append(f'<rect fill="{INK}" x="{x}" y="{shear + gap / 2}" width="9" height="{b - shear - gap / 2}"/>')
    return "".join(out)


def pawl_svg():
    """Pawl: a rack that only advances one approved tooth at a time."""
    x0, x1, base, h, p = 6, 58, 56, 9, 10.4
    pts = [(x0, base), (x0, base - 14)]
    x = x0
    while x + p <= x1 + 0.1:
        pts += [(round(x + p, 2), base - 14 - h), (round(x + p, 2), base - 14)]
        x += p
    pts[-1] = (x1, base - 14)
    pts.append((x1, base))
    vx, vy = x0 + 3 * p, base - 14
    piv = (round(vx - 13, 2), 12)
    arm = poly([(piv[0] - 4.5, piv[1] + 2), (piv[0] + 4.5, piv[1] - 2),
                (round(vx + 0.5, 2), vy - 3.2), (round(vx - 4.2, 2), vy - 6.5)])
    return (f'<path fill="{INK}" d="{poly(pts)}"/><path fill="{INK}" d="{arm}"/>'
            f'<circle fill="{INK}" cx="{piv[0]}" cy="{piv[1]}" r="6.5"/>'
            f'<circle fill="var(--ground,#fff)" cx="{piv[0]}" cy="{piv[1]}" r="2.6"/>')


def hinge_svg():
    """Hinge: two leaves on one axis; the right leaf has already swung."""
    out = [f'<rect fill="{INK}" x="8" y="10" width="19" height="44"/>',
           f'<rect fill="{INK}" x="39" y="10" width="13" height="44"/>']
    n, top, bot, g = 5, 10, 54, 2
    seg = (bot - top - g * (n - 1)) / n
    for i in range(n):
        y = top + i * (seg + g)
        x, w = (27, 9) if i % 2 == 0 else (29, 10)
        out.append(f'<rect fill="{INK}" x="{x}" y="{y:.2f}" width="{w}" height="{seg:.2f}"/>')
    return "".join(out)


def dovetail_svg():
    """Dovetail: intent and execution locked together with no fastener."""
    a = poly([(8, 10), (30, 10), (30, 21), (38, 17), (38, 47), (30, 43), (30, 54), (8, 54)])
    b = poly([(33, 10), (56, 10), (56, 54), (33, 54), (33, 45.4), (41, 49.4), (41, 14.6), (33, 18.6)])
    return f'<path fill="{INK}" d="{a}"/><path fill="{INK}" d="{b}"/>'


def firsttile_svg():
    """First tile: one release sets a deterministic chain in motion."""
    out = [f'<rect fill="{INK}" x="{x}" y="14" width="9" height="42"/>' for x in (32, 46)]
    out.append(f'<rect fill="{INK}" x="0" y="0" width="9" height="42"'
               ' transform="translate(13 56) rotate(-28) translate(0 -42)"/>')
    return "".join(out)


def ejected_svg():
    """Ejected line: one line leaves the page and becomes an object."""
    plate = "M8 8 H40 V56 H8 Z M14 17 H34 V23 H14 Z M14 29 H34 V35 H14 Z M14 41 H28 V47 H14 Z"
    return (f'<path fill="{INK}" fill-rule="evenodd" d="{plate}"/>'
            f'<rect fill="{INK}" x="44" y="29" width="14" height="6"/>')


# key, name, master, 24 px, 16 px, concept, distinctive characteristic
SYMBOLS = [
    ("indent", "Indent", indent_svg, indent_24, indent_16,
     "Three lines of code, each indented one step deeper, fuse on the right into one quarter-turn: "
     "structure on the left, a single motion on the right.",
     "A stepped inner edge against one clean arc."),
    ("heron", "Heron", heron_svg, heron_24, heron_16,
     "The heron stands still, reads the water, and strikes once, exactly: calm attention, then one "
     "precise act. The kink in its neck is the strike held in reserve until it is released.",
     "A kinked neck under a long, level bill."),
    ("halfturn", "Half-turn", halfturn_svg, halfturn_24, halfturn_16,
     "One ribbon, one half-turn. The outlined face is the plan; after the turn the same ribbon "
     "shows its solid face: the act. Nothing is lost in translation.",
     "Outline meets solid at a single crossing point."),
    ("cam", "Cam", cam_svg, None, None,
     "A cam turns rotation into a straight stroke: the circular work of reasoning, converted into "
     "one exact movement of the follower above it.",
     "An off-centre lobe with a shaft hole, and a detached follower."),
    ("tumbler", "Tumbler", tumbler_svg, None, None,
     "Pin stacks of different lengths are the key's code; when they are right, every split meets "
     "the shear line and the lock turns.",
     "Irregular columns cut by one perfectly straight line."),
    ("pawl", "Pawl", pawl_svg, None, None,
     "A rack and pawl advance one tooth at a time and never slip back: each step approved and kept.",
     "A sawtooth base with one hooked catch."),
    ("hinge", "Hinge", hinge_svg, None, None,
     "Two leaves joined on one axis by interleaved knuckles; one has begun to swing open.",
     "An interleaved seam between unequal leaves."),
    ("dovetail", "Dovetail", dovetail_svg, None, None,
     "A dovetail joint: intent and execution cut to fit each other, holding without a fastener.",
     "A flared tenon locked into its socket."),
    ("firsttile", "First tile", firsttile_svg, None, None,
     "One tile is released and the rest follow in a known order: a single approval, a deterministic chain.",
     "Two standing slabs and one leaning into them."),
    ("ejected", "Ejected line", ejected_svg, None, None,
     "One line is pushed out of the page and becomes a solid object beside it.",
     "A slotted plate with its missing piece outside."),
]

FINALISTS = ["indent", "heron", "halfturn"]


def svg(inner, view=64, size=None, color="#0b0c0e", ground="#ffffff"):
    size = size or view
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" width="{size}" '
            f'height="{size}" style="color:{color};--ground:{ground}">{inner}</svg>')


if __name__ == "__main__":
    import sys

    cells = []
    for key, name, fn, f24, f16, *_ in SYMBOLS:
        small = (svg(f24(), 24) if f24 else svg(fn(), 64, 24)) + (svg(f16(), 16) if f16 else svg(fn(), 64, 16))
        cells.append(
            f'<div class="c"><div>{svg(fn(), 64, 192)}</div><div class="s">{small}'
            f'<span style="background:#000;display:inline-flex;padding:4px">'
            f'{svg(f24() if f24 else fn(), 24 if f24 else 64, 24, "#fff", "#000")}</span>'
            f'<span style="image-rendering:pixelated">{svg(f16() if f16 else fn(), 16 if f16 else 64, 64)}</span>'
            f'</div><p>{name}</p></div>')
    html = ("<html><body style='margin:0;font:14px sans-serif'><style>.c{display:inline-block;margin:10px;"
            "border:1px solid #ddd;padding:8px}.s{display:flex;gap:10px;align-items:center}</style>"
            + "".join(cells) + "</body></html>")
    open(sys.argv[1], "w").write(html)
