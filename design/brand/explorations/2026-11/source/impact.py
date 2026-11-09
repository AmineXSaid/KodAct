"""The "from code to impact" mark: a forward arrowhead with a second stroke
entering it from the lower left. Every diagonal runs at one slope (dy = 0.8 dx)
and every cut is horizontal, so the mark reads as one constructed form.

    python3 impact.py <Outfit-Bold.ttf> <Outfit-Medium.ttf> <output folder>
"""

import sys

from build import wordmark_path

S = 0.8  # one slope for every diagonal
BLUE, BLUE_TINT = "#0a5cff", "#4d8dff"
INK, PAPER = "#0b0c0e", "#ffffff"


def construct(tip=(880, 325), apex_y=45, cut_y=172, cut_x=460, jog_y=400, jog=(505, 592),
              base_y=600, bar=(320, 445, 232, 310, 448, 492), shift=-62):
    """Return the two outlines in drawing units, from the reference image's
    proportions with every diagonal forced to slope S."""
    tx, ty = tip
    ax = tx - (ty - apex_y) / S
    cx = ax - (cut_y - apex_y) / S
    ex, fx = jog[1], jog[0]
    # inner vertex: the down-right line from the cut meets the up-right line from the jog
    vy = (ex - cut_x + (jog_y / S) + (cut_y / S)) * S / 2
    vx = cut_x + (vy - cut_y) / S
    gx = fx - (base_y - jog_y) / S
    hx = tx - (base_y - ty) / S
    big = [(cx, cut_y), (cut_x, cut_y), (vx, vy), (ex, jog_y), (fx, jog_y), (gx, base_y),
           (hx, base_y), (tx, ty), (ax, apex_y)]
    p2x, p3x, top, mid, p5x, bot = bar
    p4x = p3x + (mid - top) / S
    p6x = p5x - (bot - mid) / S
    p1x = p2x - (bot - top) / S
    small = [(x + shift, y) for x, y in
             [(p1x, bot), (p2x, top), (p3x, top), (p4x, mid), (p5x, mid), (p6x, bot)]]
    return big, small


def fit(polys, view=64, margin=6):
    xs = [x for p in polys for x, _ in p]
    ys = [y for p in polys for _, y in p]
    w, h = max(xs) - min(xs), max(ys) - min(ys)
    k = (view - 2 * margin) / max(w, h)
    ox = (view - w * k) / 2 - min(xs) * k
    oy = (view - h * k) / 2 - min(ys) * k
    return [[(x * k + ox, y * k + oy) for x, y in p] for p in polys]


def path(polys, colour, round_=0.0):
    d = " ".join("M" + " L".join(f"{x:.2f} {y:.2f}" for x, y in p) + " Z" for p in polys)
    stroke = (f' stroke="{colour}" stroke-width="{round_:.2f}" stroke-linejoin="round"' if round_ else "")
    return f'<path fill="{colour}"{stroke} d="{d}"/>'


def mark(colour, view=64, margin=6, round_=None):
    big, small = construct()
    polys = fit([big, small], view, margin)
    return path(polys, colour, round_ if round_ is not None else view * 0.02)


def svg(inner, view, title="KodAct", bg=None, rx=0):
    back = f'<rect width="{view}" height="{view}" rx="{rx}" fill="{bg}"/>' if bg else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{back}{inner}</svg>\n')


def lockup(bold, medium, kod, act, sym, tag_colour=None):
    kod_d, kod_w = wordmark_path(bold, "Kod", cap=27, tracking=-0.01)
    act_d, act_w = wordmark_path(bold, "Act", cap=27, tracking=-0.01)
    gap = 12
    x = 64 + gap - 6
    body = (f'<g>{mark(sym)}</g>'
            f'<path fill="{kod}" transform="translate({x} 41)" d="{kod_d}"/>'
            f'<path fill="{act}" transform="translate({x + kod_w + 0.6:.2f} 41)" d="{act_d}"/>')
    w = x + kod_w + 0.6 + act_w
    h = 64
    if tag_colour:
        tag_d, tag_w = wordmark_path(medium, "FROM CODE TO IMPACT", cap=5.2, tracking=0.32)
        scale = (kod_w + act_w) / tag_w
        body += (f'<path fill="{tag_colour}" transform="translate({x + 0.5} 52) scale({scale:.4f} 1)" '
                 f'd="{tag_d}"/>') if scale < 1 else (
                 f'<path fill="{tag_colour}" transform="translate({x + 0.5} 52)" d="{tag_d}"/>')
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w + 6:.2f} {h}" role="img" '
            f'aria-label="KodAct"><title>KodAct</title>{body}</svg>\n')


def main(bold, medium, out):
    files = {
        "symbol-blue.svg": svg(mark(BLUE), 64),
        "symbol-black.svg": svg(mark(INK), 64),
        "symbol-white.svg": svg(mark(PAPER), 64),
        "symbol-24.svg": svg(mark(BLUE, 24, 2, 0.35), 24),
        "symbol-16.svg": svg(mark(BLUE, 16, 1, 0.2), 16),
        "app-icon-dark.svg": svg(mark(BLUE, 64, 14), 64, bg=INK, rx=14),
        "app-icon-blue.svg": svg(mark(PAPER, 64, 14), 64, bg=BLUE, rx=14),
        "lockup-light.svg": lockup(bold, medium, INK, BLUE, BLUE),
        "lockup-dark.svg": lockup(bold, medium, PAPER, BLUE_TINT, BLUE_TINT),
        "lockup-tagline-light.svg": lockup(bold, medium, INK, BLUE, BLUE, "#5d6068"),
        "lockup-tagline-dark.svg": lockup(bold, medium, PAPER, BLUE_TINT, BLUE_TINT, "#9a9ca3"),
        "lockup-black.svg": lockup(bold, medium, INK, INK, INK),
        "lockup-white.svg": lockup(bold, medium, PAPER, PAPER, PAPER),
    }
    for name, text in files.items():
        open(f"{out}/{name}", "w").write(text)


if __name__ == "__main__":
    main(*sys.argv[1:4])
