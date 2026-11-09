"""The chosen direction: a K built from instrument parts.

A stem (the structure), a thin lever holding a weight up (intent, held until
it is approved) and a half-bowl below (where it lands: the act).

    python3 kmark.py <font.ttf> <output folder>
"""

import math
import sys

from build import wordmark_path

INK, PAPER = "#0b0c0e", "#ffffff"
COBALT, COBALT_TINT = "#1f3fd1", "#7c93ff"


def k_mark(ink, accent=None, x0=8, y0=10, W=48, H=44, sw=13, br=6, lw=1.6):
    """Draw the mark: W x H box at (x0, y0), stem width sw (also its top corner
    radius), ball radius br, lever width lw. The bowl fills the rest of the width
    and sits on the baseline; the lever leaves the stem at 45 degrees."""
    accent = accent or ink
    r = (W - sw) / 2
    by = y0 + H - r
    stem = (f"M{x0:g} {y0 + H:g} V{y0 + sw:g} A{sw:g} {sw:g} 0 0 1 {x0 + sw:g} {y0:g} V{y0 + H:g} Z")
    bowl = f"M{x0 + sw:g} {by:g} H{x0 + W:g} A{r:g} {r:g} 0 0 1 {x0 + sw:g} {by:g} Z"
    t = by - (y0 + br)
    bx, bcy = x0 + sw + t, y0 + br
    lev = (f'<path fill="none" stroke="{ink}" stroke-width="{lw:g}" '
           f'd="M{x0 + sw - lw:g} {by + lw:g} L{bx:g} {bcy:g}"/>')
    return (f'<path fill="{ink}" d="{stem} {bowl}"/>' + lev +
            f'<circle fill="{accent}" cx="{bx:g}" cy="{bcy:g}" r="{br:g}"/>')


def mark_64(ink, accent=None):
    return k_mark(ink, accent)


def mark_24(ink, accent=None):
    # Whole pixels: stem 5 px, bowl radius 7, ball radius 2.5, lever 1.5 px.
    return k_mark(ink, accent, x0=3, y0=3, W=19, H=18, sw=5, br=2.5, lw=1.5)


def mark_16(ink, accent=None):
    # Whole pixels: stem 3 px, bowl radius 4.5, ball radius 2, lever 1 px.
    return k_mark(ink, accent, x0=2, y0=2, W=12, H=12, sw=3, br=2, lw=1.1)


def doc(inner, view, title):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{inner}</svg>\n')


def lockup(word_d, word_w, ink, accent, word_ink):
    gap = 13
    inner = (f'<g transform="translate(-8 0)">{mark_64(ink, accent)}</g>'
             f'<path fill="{word_ink}" transform="translate({48 + gap} 45)" d="{word_d}"/>')
    w = 48 + gap + word_w
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="-8 0 {w + 16:.2f} 64" role="img" '
            f'aria-label="KodAct"><title>KodAct</title>{inner}</svg>\n')


def main(font, out):
    word_d, word_w = wordmark_path(font, cap=27, tracking=-0.005)
    files = {
        "symbol-black.svg": doc(mark_64(INK), 64, "KodAct"),
        "symbol-white.svg": doc(mark_64(PAPER), 64, "KodAct"),
        "symbol-colour.svg": doc(mark_64(INK, COBALT), 64, "KodAct"),
        "symbol-colour-on-dark.svg": doc(mark_64(PAPER, COBALT_TINT), 64, "KodAct"),
        "symbol-24.svg": doc(mark_24(INK), 24, "KodAct"),
        "symbol-16.svg": doc(mark_16(INK), 16, "KodAct"),
        "symbol-24-white.svg": doc(mark_24(PAPER), 24, "KodAct"),
        "symbol-16-white.svg": doc(mark_16(PAPER), 16, "KodAct"),
        "lockup-black.svg": lockup(word_d, word_w, INK, INK, INK),
        "lockup-white.svg": lockup(word_d, word_w, PAPER, PAPER, PAPER),
        "lockup-colour.svg": lockup(word_d, word_w, INK, COBALT, INK),
        "lockup-colour-on-dark.svg": lockup(word_d, word_w, PAPER, COBALT_TINT, PAPER),
    }
    for name, text in files.items():
        open(f"{out}/{name}", "w").write(text)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
