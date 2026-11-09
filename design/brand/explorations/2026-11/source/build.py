"""Build the November 2026 identity exploration from symbols.py.

    pip install fonttools uharfbuzz
    # InstrumentSans-SemiBold.ttf (SIL OFL 1.1, from Google Fonts)
    python3 build.py <font.ttf> <output folder>

Writes the ten concept SVGs, every finalist file (symbol in black, white and
colour, the 16 and 24 px drawings, horizontal lockups) and the presentation
page. `node render.js <output folder>` then renders the finalists' PNGs.
"""

import html
import os
import sys

import uharfbuzz as hb
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

import symbols as S

INK = "#0b0c0e"
PAPER = "#ffffff"

# One signature colour per finalist, with a lighter tint for dark grounds.
COLOURS = {
    "indent": ("Cobalt", "#1f3fd1", "#7c93ff",
               "A saturated, slightly warm cobalt: the blue of engineering drawings and enamel signs, "
               "not of a startup gradient. It carries the single sweep of the mark."),
    "heron": ("Night indigo", "#2a2670", "#9b97f0",
              "A deep blue-violet, close to ink: calm and serious, the colour of water at dusk where the "
              "heron waits. Reads as near-black at a distance, as colour up close."),
    "halfturn": ("Kiln", "#b23c17", "#f08a63",
                 "A fired-clay red-orange: warm, physical, the colour of a material that has been through "
                 "a process. It marks the solid face, the act."),
}

VERDICTS = {
    "cam": "Strong idea, but at 16 px the lobe and follower read as a letter d.",
    "tumbler": "The meaning is exact; the silhouette reads as an audio equaliser.",
    "pawl": "Too many parts; the teeth read as a factory roofline.",
    "hinge": "The interleaved seam reads as the letter E.",
    "dovetail": "Collapses into a door or a pair of panels below 32 px.",
    "firsttile": "The leaning tile reads as a slash, a coding character.",
    "ejected": "Reads as a document or menu icon: generic interface vocabulary.",
}


# --------------------------------------------------------------------------
# Wordmark: "KodAct" set in Instrument Sans SemiBold and turned into outlines.
# --------------------------------------------------------------------------
def wordmark_path(font_path, text="KodAct", cap=26.0, tracking=-0.012):
    """Return (svg path d, advance width) with the baseline at y=0 and the cap
    height scaled to `cap` units."""
    blob = hb.Blob.from_file_path(font_path)
    face = hb.Face(blob)
    font = hb.Font(face)
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, {"kern": True, "liga": False})

    tt = TTFont(font_path)
    upem = tt["head"].unitsPerEm
    cap_units = tt["OS/2"].sCapHeight
    k = cap / cap_units
    glyphs = tt.getGlyphSet()
    order = tt.getGlyphOrder()

    x = 0.0
    parts = []
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        name = order[info.codepoint]
        pen = SVGPathPen(glyphs)
        glyphs[name].draw(TransformPen(pen, (k, 0, 0, -k, x + pos.x_offset * k, -pos.y_offset * k)))
        parts.append(pen.getCommands())
        x += pos.x_advance * k + tracking * upem * k
    x -= tracking * upem * k
    return " ".join(parts), x


def svg_doc(inner, w, h, title, extra=""):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:g} {h:g}" role="img" '
            f'aria-label="{title}"{extra}><title>{title}</title>{inner}</svg>\n')


def solid(inner, colour):
    """Make a currentColor drawing standalone in one colour."""
    return f'<g style="color:{colour};fill:{colour}">{inner.replace("var(--ground,#fff)", "none")}</g>'


def lockup(sym_inner, word_d, word_w, sym_colour, word_colour, bg=None):
    """Horizontal lockup: symbol (48 units of artwork) + wordmark with a cap
    height of 26 units, both centred on y=32, 14 units apart."""
    gap = 14
    sym = f'<g transform="translate(-8 0)">{solid(sym_inner, sym_colour)}</g>'
    word = f'<path style="fill:{word_colour}" transform="translate({48 + gap} 45)" d="{word_d}"/>'
    w = 48 + gap + word_w
    pad = 8
    back = f'<rect x="{-pad}" y="0" width="{w + 2 * pad:.2f}" height="64" fill="{bg}"/>' if bg else ""
    return back + sym + word, w, pad


def main(font_path, out):
    os.makedirs(f"{out}/concepts", exist_ok=True)
    word_d, word_w = wordmark_path(font_path)

    by_key = {s[0]: s for s in S.SYMBOLS}

    # Ten concepts, black on white.
    for i, (key, name, fn, *_rest) in enumerate(S.SYMBOLS, 1):
        inner = f'<rect width="64" height="64" fill="{PAPER}"/>' + solid(fn(), INK)
        open(f"{out}/concepts/{i:02d}-{key}.svg", "w").write(svg_doc(inner, 64, 64, f"KodAct concept: {name}"))

    # Finalists.
    for key in S.FINALISTS:
        _, name, fn, f24, f16, *_ = by_key[key]
        cname, hexc, tint, _why = COLOURS[key]
        d = f"{out}/finalists/{key}"
        os.makedirs(d, exist_ok=True)
        files = {
            "symbol-black.svg": (solid(fn(), INK), 64),
            "symbol-white.svg": (solid(fn(), PAPER), 64),
            "symbol-colour.svg": (solid(fn(), hexc), 64),
            "symbol-24.svg": (solid(f24(), INK), 24),
            "symbol-24-white.svg": (solid(f24(), PAPER), 24),
            "symbol-16.svg": (solid(f16(), INK), 16),
            "symbol-16-white.svg": (solid(f16(), PAPER), 16),
        }
        for fname, (inner, view) in files.items():
            open(f"{d}/{fname}", "w").write(svg_doc(inner, view, view, f"KodAct symbol, {name}"))
        for fname, args in {
            "lockup-black.svg": (INK, INK, None),
            "lockup-white.svg": (PAPER, PAPER, None),
            "lockup-colour.svg": (hexc, INK, None),
            "lockup-colour-on-dark.svg": (tint, PAPER, None),
        }.items():
            inner, w, pad = lockup(fn(), word_d, word_w, *args)
            doc = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{-pad} 0 {w + 2 * pad:.2f} 64" '
                   f'role="img" aria-label="KodAct"><title>KodAct</title>{inner}</svg>\n')
            open(f"{d}/{fname}", "w").write(doc)

    open(f"{out}/presentation.html", "w").write(presentation(word_d, word_w))


# --------------------------------------------------------------------------
# Presentation page
# --------------------------------------------------------------------------
def inline(inner, view, size, colour, cls=""):
    return (f'<svg class="{cls}" viewBox="0 0 {view} {view}" width="{size}" height="{size}" aria-hidden="true">'
            f'{solid(inner, colour)}</svg>')


def inline_lockup(fn, word_d, word_w, sym_colour, word_colour, height):
    inner, w, pad = lockup(fn(), word_d, word_w, sym_colour, word_colour)
    vw = w + 2 * pad
    return (f'<svg class="lock" viewBox="{-pad} 0 {vw:.2f} 64" style="height:{height}px;width:auto;max-width:100%" '
            f'role="img" aria-label="KodAct">{inner}</svg>')


def pixel(inner, view, colour):
    """Placeholder canvas the page script fills with a true-size raster,
    magnified without smoothing."""
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view} {view}" width="{view}" '
           f'height="{view}">{solid(inner, colour)}</svg>')
    return f'<canvas class="px" data-size="{view}" data-svg="{html.escape(svg)}"></canvas>'


def presentation(word_d, word_w):
    by_key = {s[0]: s for s in S.SYMBOLS}
    concepts = []
    for i, (key, name, fn, f24, f16, concept, trait) in enumerate(S.SYMBOLS, 1):
        final = key in S.FINALISTS
        small = (inline(f24(), 24, 24, INK) if f24 else inline(fn(), 64, 24, INK)) + (
            inline(f16(), 16, 16, INK) if f16 else inline(fn(), 64, 16, INK))
        verdict = ("Finalist" if final else VERDICTS.get(key, ""))
        concepts.append(f"""
      <article class="concept{' is-final' if final else ''}">
        <div class="plate">{inline(fn(), 64, 160, INK, 'big')}<div class="tiny">{small}</div></div>
        <div class="meta">
          <p class="idx">{i:02d}{'<span class="tag">Finalist</span>' if final else ''}</p>
          <h3>{name}</h3>
          <p>{concept}</p>
          <p class="trait"><span>Distinctive</span>{trait}</p>
          {'' if final else f'<p class="verdict"><span>Why not</span>{verdict}</p>'}
        </div>
      </article>""")

    finals = []
    for n, key in enumerate(S.FINALISTS):
        _, name, fn, f24, f16, concept, trait = by_key[key]
        cname, hexc, tint, why = COLOURS[key]
        rank = ["Recommended", "Second", "Third"][n]
        finals.append(f"""
    <section class="final" id="{key}">
      <header class="fhead">
        <p class="eyebrow">{rank}</p>
        <h2>{name}</h2>
        <p class="lede">{concept}</p>
      </header>
      <div class="pair">
        <figure class="tile light">{inline(fn(), 64, 256, INK, 'sym256')}<figcaption>Black on white · 256 px</figcaption></figure>
        <figure class="tile dark">{inline(fn(), 64, 256, PAPER, 'sym256')}<figcaption>White on black · 256 px</figcaption></figure>
      </div>
      <div class="sizes">
        <div class="sizecol">
          <p class="label">Actual size</p>
          <div class="row-true">
            <span class="chip light">{inline(f24(), 24, 24, INK)}</span>
            <span class="chip light">{inline(f16(), 16, 16, INK)}</span>
            <span class="chip dark">{inline(f24(), 24, 24, PAPER)}</span>
            <span class="chip dark">{inline(f16(), 16, 16, PAPER)}</span>
          </div>
          <p class="note">Each small size is its own drawing, fitted to the pixel grid.</p>
        </div>
        <div class="sizecol">
          <p class="label">Pixel test · 24 px and 16 px, magnified 6×</p>
          <div class="row-px">
            <span class="pxw light">{pixel(f24(), 24, INK)}</span>
            <span class="pxw light">{pixel(f16(), 16, INK)}</span>
            <span class="pxw dark">{pixel(f24(), 24, PAPER)}</span>
            <span class="pxw dark">{pixel(f16(), 16, PAPER)}</span>
          </div>
        </div>
      </div>
      <div class="lockups">
        <div class="lockrow light">{inline_lockup(fn, word_d, word_w, INK, INK, 88)}</div>
        <div class="lockrow dark">{inline_lockup(fn, word_d, word_w, PAPER, PAPER, 88)}</div>
      </div>
      <div class="colour">
        <div class="swatch" style="background:{hexc}"><span>{cname}</span><code>{hexc}</code></div>
        <div class="colourtext">
          <p class="label">Signature colour</p>
          <p>{why}</p>
          <p class="note">On dark grounds the symbol takes the lighter tint <code>{tint}</code>.</p>
        </div>
        <div class="lockrow light">{inline_lockup(fn, word_d, word_w, hexc, INK, 56)}</div>
        <div class="lockrow dark">{inline_lockup(fn, word_d, word_w, tint, PAPER, 56)}</div>
      </div>
    </section>""")

    indent = by_key["indent"][2]
    return PAGE.format(
        hero=inline_lockup(indent, word_d, word_w, "var(--fg)", "var(--fg)", 88),
        concepts="".join(concepts),
        finals="".join(finals),
    )


PAGE = """<title>KodAct Identity</title>
<meta name="claude:viewport-fit-sides" content="cover">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500&display=swap">
<style>
/* Layout: a quiet specimen book. Black-on-white plates are fixed specimens; the page around them follows the theme. */
:root {{
  --bg: #f4f4f2; --fg: #0b0c0e; --muted: #5d6068; --rule: #d9d9d4; --card: #ffffff; --accent: #1f3fd1;
  --plate: #ffffff; --plate-ink: #0b0c0e; --night: #0b0c0e; --night-ink: #ffffff;
  --sans: "Instrument Sans", "Helvetica Neue", Arial, sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, "SFMono-Regular", Menlo, monospace;
}}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{
  --bg: #111215; --fg: #ececea; --muted: #9a9ca3; --rule: #2a2c31; --card: #18191d; --accent: #7c93ff; color-scheme: dark; }} }}
:root[data-theme="dark"] {{ --bg: #111215; --fg: #ececea; --muted: #9a9ca3; --rule: #2a2c31; --card: #18191d; --accent: #7c93ff; color-scheme: dark; }}
* {{ box-sizing: border-box; }}
body {{ background: var(--bg); color: var(--fg); font: 400 16px/1.55 var(--sans); }}
.wrap {{ max-width: 1120px; margin: 0 auto; padding-block: 56px 96px;
  padding-left: max(16px, env(safe-area-inset-left, 0px)); padding-right: max(16px, env(safe-area-inset-right, 0px)); }}
h1, h2, h3 {{ font-weight: 600; letter-spacing: -0.015em; text-wrap: balance; margin: 0; }}
h2 {{ font-size: clamp(28px, 4vw, 40px); }}
h3 {{ font-size: 20px; }}
p {{ margin: 0; }}
code {{ font-family: var(--mono); font-size: 0.9em; }}
.eyebrow, .label, .idx {{ font: 500 12px/1.4 var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted); }}
.note {{ color: var(--muted); font-size: 14px; }}
.hero {{ display: grid; grid-template-columns: minmax(0, 1fr); gap: 28px; padding-bottom: 56px; border-bottom: 1px solid var(--rule); }}
.hero .lock {{ color: var(--fg); }}
.thesis {{ font-size: clamp(22px, 3vw, 30px); line-height: 1.3; max-width: 30ch; letter-spacing: -0.01em; }}
.intro {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 24px; color: var(--muted); max-width: 900px; }}
.intro p {{ min-width: 0; }}
.section-head {{ display: grid; gap: 10px; padding-block: 56px 28px; }}
.section-head p:last-child {{ color: var(--muted); max-width: 62ch; }}
.concepts {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 1px; background: var(--rule); border: 1px solid var(--rule); }}
.concept {{ background: var(--card); display: grid; grid-template-rows: auto 1fr; min-width: 0; }}
.plate {{ background: var(--plate); color: var(--plate-ink); display: flex; align-items: flex-end; justify-content: space-between; padding: 20px; gap: 12px; border-bottom: 1px solid var(--rule); }}
.plate .big {{ width: 160px; height: 160px; max-width: 60%; }}
.tiny {{ display: flex; gap: 10px; align-items: flex-end; }}
.meta {{ padding: 18px 20px 22px; display: grid; gap: 8px; align-content: start; }}
.meta > p:not(.idx) {{ font-size: 15px; }}
.idx {{ display: flex; gap: 10px; align-items: center; }}
.tag {{ color: var(--accent); }}
.trait span, .verdict span {{ display: block; font: 500 11px/1.4 var(--mono); letter-spacing: 0.08em; text-transform: uppercase; color: var(--muted); margin-bottom: 2px; }}
.verdict {{ color: var(--muted); }}
.is-final .plate {{ box-shadow: inset 0 -3px 0 var(--accent); }}
.final {{ padding-block: 64px; border-top: 1px solid var(--rule); display: grid; gap: 28px; }}
.fhead {{ display: grid; gap: 10px; }}
.lede {{ font-size: 18px; max-width: 60ch; }}
.pair {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 16px; }}
.tile {{ margin: 0; aspect-ratio: 1 / 1; max-width: 100%; display: grid; place-items: center; position: relative; }}
.tile.light, .light {{ background: var(--plate); color: var(--plate-ink); }}
.tile.dark, .dark {{ background: var(--night); color: var(--night-ink); }}
.tile.light, .tile.dark {{ outline: 1px solid var(--rule); outline-offset: -1px; }}
.sym256 {{ width: min(256px, 70%); height: auto; }}
figcaption {{ position: absolute; left: 16px; bottom: 12px; font: 500 11px/1.4 var(--mono); letter-spacing: 0.06em; text-transform: uppercase; opacity: 0.6; }}
.sizes {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 24px; }}
.sizecol {{ display: grid; gap: 12px; align-content: start; min-width: 0; }}
.row-true, .row-px {{ display: flex; flex-wrap: wrap; gap: 10px; align-items: flex-end; }}
.chip {{ display: inline-grid; place-items: center; width: 48px; height: 48px; outline: 1px solid var(--rule); outline-offset: -1px; }}
.pxw {{ display: inline-grid; place-items: center; padding: 10px; outline: 1px solid var(--rule); outline-offset: -1px; }}
canvas.px {{ image-rendering: pixelated; display: block; }}
.lockups {{ display: grid; gap: 16px; }}
.lockrow {{ padding: 36px 28px; display: flex; align-items: center; overflow: hidden; }}
.lockrow.light, .lockrow.dark {{ outline: 1px solid var(--rule); outline-offset: -1px; }}
.colour {{ display: grid; grid-template-columns: minmax(0, 200px) minmax(0, 1fr); gap: 16px 24px; align-items: start; }}
.colour .lockrow {{ grid-column: 1 / -1; }}
.swatch {{ aspect-ratio: 1 / 1; color: #fff; padding: 14px; display: flex; flex-direction: column; justify-content: flex-end; gap: 2px; font-weight: 600; }}
.swatch code {{ font-weight: 400; opacity: 0.85; }}
.colourtext {{ display: grid; gap: 8px; align-content: start; max-width: 60ch; }}
.closing {{ padding-top: 64px; border-top: 1px solid var(--rule); display: grid; gap: 16px; max-width: 70ch; }}
.closing ol {{ margin: 0; padding-left: 1.2em; display: grid; gap: 8px; }}
@media (max-width: 560px) {{
  .colour {{ grid-template-columns: 1fr; }}
  .swatch {{ aspect-ratio: auto; height: 120px; }}
  .lockrow {{ padding: 24px 16px; }}
  .lockrow .lock, .hero .lock {{ height: auto !important; width: 100% !important; }}
}}
</style>

<main class="wrap">
  <header class="hero">
    <p class="eyebrow">Identity exploration · November 2026</p>
    {hero}
    <p class="thesis">Code becomes action. Ten ideas for the moment written intent turns into a precise, permitted act.</p>
    <div class="intro">
      <p>Every symbol was drawn in solid black first, on a 64-unit grid with an 8-unit margin. No letters, arrows,
      brackets or code characters; no gradients or effects.</p>
      <p>The three finalists were redrawn for 24 and 16 pixels, tested in white on black, set with the wordmark,
      and given one signature colour each. The pixel tests below are true-size renders, magnified.</p>
    </div>
  </header>

  <div class="section-head">
    <p class="eyebrow">Ten directions</p>
    <h2>Symbol concepts</h2>
    <p>Each is a different idea, not a variation. The small marks beside each symbol are the same drawing at 24 and
    16 px; where a concept failed at that size or read as something else, the note says so.</p>
  </div>
  <div class="concepts">{concepts}
  </div>

  <div class="section-head">
    <p class="eyebrow">Three finalists</p>
    <h2>Developed directions</h2>
    <p>The wordmark is Instrument Sans SemiBold with tracking tightened by 1.2%, converted to outlines. The
    symbol's artwork is 48 units tall; the wordmark's cap height is 26 units, centred on the symbol, 14 units away.</p>
  </div>
  {finals}

  <section class="closing">
    <p class="eyebrow">Recommendation</p>
    <h2>Indent</h2>
    <p>Indent says the brand in its construction: indentation is the most recognisable trace of code, and here it
    resolves into one arc. It is the only finalist that is unambiguous at 16 px, it has no part that reads as a
    letter, and its silhouette (stepped inside, swept outside) has no near neighbour among developer tools.
    Heron is the most characterful; Half-turn the most conceptual.</p>
    <ol>
      <li>Choose a direction.</li>
      <li>Refine it: optical corrections, a reversed cut with thinner gaps, and construction drawings.</li>
      <li>Build the kit: Windows .ico (16 to 256 px), favicon, CLI banner, lockups and a usage page.</li>
    </ol>
  </section>
</main>

<script>
document.querySelectorAll("canvas.px").forEach(function (c) {{
  var size = +c.dataset.size, scale = 6, bg = c.closest(".dark") ? "#0b0c0e" : "#ffffff";
  c.width = c.height = size * scale;
  c.style.width = c.style.height = size * scale + "px";
  var t = document.createElement("canvas"); t.width = t.height = size;
  var tx = t.getContext("2d"); tx.fillStyle = bg; tx.fillRect(0, 0, size, size);
  var img = new Image();
  img.onload = function () {{
    tx.drawImage(img, 0, 0, size, size);
    var g = c.getContext("2d"); g.imageSmoothingEnabled = false; g.drawImage(t, 0, 0, size * scale, size * scale);
  }};
  img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(c.dataset.svg);
}});
</script>
"""


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
