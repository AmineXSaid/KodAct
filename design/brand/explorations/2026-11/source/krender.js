const { chromium } = require('playwright'); const fs=require('fs'), path=require('path');
(async()=>{const d=process.argv[2]; const b=await chromium.launch(); const p=await b.newPage();
for (const [f,h,w] of [['symbol-16.svg',16,16],['symbol-24.svg',24,24],['symbol-black.svg',256,256],['symbol-white.svg',256,256],['symbol-colour.svg',256,256],['lockup-black.svg',128,0],['lockup-colour.svg',128,0],['lockup-colour-on-dark.svg',128,0]]) {
 await p.setContent(`<style>body{margin:0}svg{display:block;height:${h}px;width:${w?w+'px':'auto'}}</style>`+fs.readFileSync(path.join(d,f),'utf8'));
 await (await p.$('svg')).screenshot({path:path.join(d,'png',f.replace('.svg',`-${h}.png`).replace(/-(16|24)-\1/,'-$1')),omitBackground:true});}
await b.close();})();
