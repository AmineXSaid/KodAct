# Chosen direction: the K

A K built from instrument parts: a stem (the structure), a thin lever holding
a weight up (intent, held until it is approved) and a half-bowl below, where it
lands (the act). Picked from an image-generator run and redrawn here as clean
geometry. It breaks the brief's "no letters" rule by the owner's choice.

| File | Use |
| --- | --- |
| `symbol-black.svg`, `symbol-white.svg` | Master symbol, 64-unit grid |
| `symbol-colour.svg`, `symbol-colour-on-dark.svg` | Weight in cobalt `#1f3fd1` (tint `#7c93ff` on dark) |
| `symbol-24.svg`, `symbol-16.svg` (and `-white`) | Separate drawings on whole pixels |
| `lockup-*.svg` | Symbol + wordmark, horizontal |
| `png/` | Renders |

Wordmark: Jost Medium (SIL OFL 1.1), stored as outlines. Rebuild from
`../source`: `python3 kmark.py <Jost-Medium.ttf> ../k`, then
`node krender.js ../k` for the PNGs.

Not yet done: the Windows `.ico`, favicon, and replacing the files in
`design/brand/`.
