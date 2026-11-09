# Direction: from code to impact

A forward arrowhead with a second stroke entering it from the lower left:
code coming in, leaving as one direction. Redrawn flat from an image-generator
reference: every diagonal at one slope (dy = 0.8 dx), every cut horizontal, no
gradient or glow.

| File | Use |
| --- | --- |
| `symbol-blue.svg`, `symbol-black.svg`, `symbol-white.svg` | Symbol, 64-unit grid |
| `symbol-24.svg`, `symbol-16.svg` | Small sizes |
| `app-icon-dark.svg`, `app-icon-blue.svg` | App icon tiles |
| `lockup-light.svg`, `lockup-dark.svg` | Symbol + "Kod" in ink or white, "Act" in blue |
| `lockup-tagline-*.svg` | The same with FROM CODE TO IMPACT |
| `lockup-black.svg`, `lockup-white.svg` | One colour |
| `png/` | Renders |

Blue `#0a5cff` (tint `#4d8dff` on dark). Type: Outfit Bold and Medium (SIL OFL
1.1), as outlines. Rebuild from `../source`:
`python3 impact.py <Outfit-Bold.ttf> <Outfit-Medium.ttf> ../impact`, then
`node impact-render.js ../impact`.
