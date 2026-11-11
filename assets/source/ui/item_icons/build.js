// Regenerates atlas.png, atlas.txt and sheet.png from svg/ (made by gen_svgs.py) and items.tsv. Run: python3 gen_svgs.py && node build.js; then copy atlas.png and atlas.txt to assets/ui/item_icons.png and .txt.
const fs = require('fs'), path = require('path');
const { chromium } = require('playwright');
const here = __dirname;
const rows = fs.readFileSync(path.join(here, 'items.tsv'), 'utf8').split('\n').filter(l => l.trim()).map(l => l.split('\t'));
const svg = id => fs.readFileSync(path.join(here, 'svg', id + '.svg'), 'utf8').trim();
const uri = id => 'data:image/svg+xml;base64,' + Buffer.from(svg(id)).toString('base64');
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 960, height: 96 }, deviceScaleFactor: 1 });
  // atlas: 10 columns of 96px, transparent
  const cols = 10, n = rows.length, nrows = Math.ceil(n / cols);
  let html = `<html><body style="margin:0;background:transparent"><div style="display:grid;grid-template-columns:repeat(${cols},96px);width:960px">` +
    rows.map(r => `<img src="${uri(r[0])}" width="96" height="96" style="display:block">`).join('') + '</div></body></html>';
  await page.setViewportSize({ width: 960, height: 96 * nrows });
  await page.setContent(html);
  await page.screenshot({ path: path.join(here, 'atlas.png'), omitBackground: true, clip: { x: 0, y: 0, width: 960, height: 96 * nrows } });
  fs.writeFileSync(path.join(here, 'atlas.txt'), rows.map(r => r[0]).join('\n') + '\n');
  // contact sheet
  const cats = []; rows.forEach(r => { if (!cats.includes(r[2])) cats.push(r[2]); });
  let s = `<html><body style="margin:0;background:#141414;width:1920px;font-family:'Helvetica Neue',Arial,sans-serif;color:#f2e6c9">
  <style>h2{font:600 18px 'Helvetica Neue',Arial,sans-serif;letter-spacing:.08em;text-transform:uppercase;margin:26px 32px 12px;color:#8db06b}
  .g{display:flex;flex-wrap:wrap;gap:12px;margin:0 32px}.t{width:120px;text-align:center}
  .i{width:96px;height:96px;margin:0 auto;background:#1b1c1c;border:1px solid rgba(242,230,201,.14);border-radius:6px;display:grid;place-items:center}
  .i img{width:72px;height:72px}.n{font-size:14px;margin-top:6px;color:#e9e0c6;line-height:1.2}.id{font-size:10px;color:#7d776a}</style><div style="height:16px"></div>`;
  for (const c of cats) {
    s += `<h2>${c}</h2><div class="g">` + rows.filter(r => r[2] === c).map(r => `<div class="t"><div class="i"><img src="${uri(r[0])}"></div><div class="n">${r[1]}</div></div>`).join('') + '</div>';
  }
  s += '<div style="height:30px"></div></body></html>';
  const p2 = await browser.newPage({ viewport: { width: 1920, height: 1000 }, deviceScaleFactor: 1 });
  await p2.setContent(s);
  await p2.screenshot({ path: path.join(here, 'sheet.png'), fullPage: true });
  await browser.close();
  console.log('atlas', 960, 'x', 96 * nrows, 'icons', n);
})();
