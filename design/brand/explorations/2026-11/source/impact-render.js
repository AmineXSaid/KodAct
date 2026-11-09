// Render the impact mark's files to PNG. Usage: node impact-render.js <impact folder>
const { chromium } = require('playwright'); const fs = require('fs'), path = require('path');
(async () => {
  const d = process.argv[2]; const b = await chromium.launch(); const p = await b.newPage();
  const jobs = [['symbol-16.svg', 16, 16], ['symbol-24.svg', 24, 24], ['symbol-blue.svg', 256, 256],
    ['symbol-black.svg', 256, 256], ['symbol-white.svg', 256, 256], ['app-icon-dark.svg', 256, 256],
    ['app-icon-blue.svg', 256, 256], ['lockup-tagline-light.svg', 160, 0], ['lockup-tagline-dark.svg', 160, 0],
    ['lockup-light.svg', 160, 0], ['lockup-dark.svg', 160, 0]];
  for (const [f, h, w] of jobs) {
    await p.setContent(`<style>body{margin:0}svg{display:block;height:${h}px;width:${w ? w + 'px' : 'auto'}}</style>` +
      fs.readFileSync(path.join(d, f), 'utf8'));
    const name = /-(16|24)\.svg$/.test(f) ? f.replace('.svg', '.png') : f.replace('.svg', `-${h}.png`);
    await (await p.$('svg')).screenshot({ path: path.join(d, 'png', name), omitBackground: true });
  }
  await b.close();
})();
