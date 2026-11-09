// Render each finalist's symbols to PNG at the sizes in their file names.
// Usage: node render.js <output folder>   (needs Playwright with Chromium)
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');
(async () => {
  const out = process.argv[2];
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const key of fs.readdirSync(path.join(out, 'finalists'))) {
    const dir = path.join(out, 'finalists', key);
    const jobs = [['symbol-16.svg', 16], ['symbol-24.svg', 24], ['symbol-black.svg', 256],
                  ['symbol-white.svg', 256], ['symbol-colour.svg', 256], ['lockup-black.svg', 0]];
    for (const [file, size] of jobs) {
      const svg = fs.readFileSync(path.join(dir, file), 'utf8');
      const h = size || 128;
      await page.setContent(`<style>html,body{margin:0;background:transparent}svg{display:block;height:${h}px;width:${size ? size + 'px' : 'auto'}}</style>${svg}`);
      const el = await page.$('svg');
      const name = file.replace('.svg', '') + (size ? (file.includes(String(size)) ? '' : `-${size}`) : `-${h}`) + '.png';
      await el.screenshot({ path: path.join(out, 'png', `${key}-${name}`), omitBackground: true });
    }
  }
  await browser.close();
})();
