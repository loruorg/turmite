// Builds the logo files in public/ from the mark geometry and the Inter outlines.
// Run: pnpm brand   (writes public/*.svg, favicon.ico, apple-touch-icon.png, og.png)
// Pass --preview DIR to also write PNG previews at several sizes.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import * as fontkit from 'fontkit';
import wawoff2 from 'wawoff2';
import { Resvg } from '@resvg/resvg-js';
import sharp from 'sharp';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pub = path.join(root, 'public');
const previewDir = process.argv.includes('--preview') ? process.argv[process.argv.indexOf('--preview') + 1] : null;

// The mark: a 3 x 3 grid, cell 4, gap 1. Black cells form a T. The last cell is the turmite head.
const CELL = 4;
const GAP = 1;
const P = CELL + GAP;
const SIZE = 3 * CELL + 2 * GAP; // 14
function markPath(ox = 0, oy = 0, s = 1) {
  const sq = (cx, cy) => {
    const x = ox + cx * P * s, y = oy + cy * P * s, c = CELL * s;
    return `M${n(x)} ${n(y)}h${n(c)}v${n(c)}h${n(-c)}z`;
  };
  const head = () => {
    const x = ox + 1 * P * s, y = oy + 2 * P * s, c = CELL * s, r = c / 2;
    return `M${n(x)} ${n(y)}h${n(c)}v${n(r)}a${n(r)} ${n(r)} 0 0 1 ${n(-c)} 0z`;
  };
  return sq(0, 0) + sq(1, 0) + sq(2, 0) + sq(1, 1) + head();
}
function n(v) {
  return Number(v.toFixed(3)).toString();
}

// Wordmark outlines: Inter, weight 600, tracking -0.025em.
const interWoff2 = path.join(root, 'node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2');
const interTtf = Buffer.from(await wawoff2.decompress(fs.readFileSync(interWoff2)));
const inter = fontkit.create(interTtf);
function textPath(str, { weight = 600, size = 100, x = 0, y = 0, tracking = -0.025 } = {}) {
  const f = inter.getVariation({ wght: weight });
  const scale = size / f.unitsPerEm;
  const run = f.layout(str);
  let cx = 0;
  let d = '';
  run.glyphs.forEach((g, i) => {
    const pos = run.positions[i];
    d += g.path.scale(scale, -scale).translate(x + (cx + pos.xOffset) * scale, y).toSVG();
    cx += pos.xAdvance + tracking * f.unitsPerEm;
  });
  return { d, width: (cx - tracking * f.unitsPerEm) * scale, capHeight: f.capHeight * scale, xHeight: f.xHeight * scale };
}

// logo.svg: the mark alone, currentColor.
const logo = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${SIZE} ${SIZE}" fill="currentColor"><path d="${markPath()}"/></svg>\n`;

// favicon.svg: the mark on the 16 x 16 pixel grid, 1 px margin. The head is rounded in whole pixels.
// Black, or white when the browser UI is dark.
const FAVICON_D = 'M1 1h4v4H1zM6 1h4v4H6zM11 1h4v4h-4zM6 6h4v4H6zM6 11h4v3H6zM7 14h2v1H7z';
const favicon = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><style>path{fill:#000}@media (prefers-color-scheme:dark){path{fill:#fff}}</style><path d="${FAVICON_D}"/></svg>\n`;

// logo-wordmark.svg: the mark + "turmite". The mark height equals the cap height; it sits on the baseline.
const FS = 100;
const probe = textPath('turmite', { size: FS });
const markScale = probe.capHeight / SIZE;
const markW = SIZE * markScale;
const gapW = markW * 0.34;
const word = textPath('turmite', { size: FS, x: markW + gapW, y: 0 });
const top = -probe.capHeight;
const descent = 0; // no descenders in "turmite"
const pad = 0;
const wmW = markW + gapW + word.width;
const wmH = probe.capHeight + descent;
const wordmark = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${n(-pad)} ${n(top - pad)} ${n(wmW + 2 * pad)} ${n(wmH + 2 * pad)}" fill="currentColor"><path d="${markPath(0, top, markScale)}"/><path d="${word.d}"/></svg>\n`;

fs.writeFileSync(path.join(pub, 'logo.svg'), logo);
fs.writeFileSync(path.join(pub, 'favicon.svg'), favicon);
fs.writeFileSync(path.join(pub, 'logo-wordmark.svg'), wordmark);

function png(svg, width, background) {
  return new Resvg(svg, { fitTo: { mode: 'width', value: width }, background }).render().asPng();
}
const solid = (svg, color) => svg.replace('fill="currentColor"', `fill="${color}"`);

// favicon.ico: PNG entries at 16, 32 and 48, drawn from the pixel-aligned favicon.
const faviconBlack = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" shape-rendering="crispEdges"><path fill="#000" d="${FAVICON_D}"/></svg>`;
const icoSizes = [16, 32, 48];
const icoImages = icoSizes.map((s) => png(faviconBlack, s));
const header = Buffer.alloc(6 + 16 * icoImages.length);
header.writeUInt16LE(0, 0);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(icoImages.length, 4);
let offset = header.length;
icoImages.forEach((img, i) => {
  const e = 6 + 16 * i;
  header.writeUInt8(icoSizes[i] % 256, e);
  header.writeUInt8(icoSizes[i] % 256, e + 1);
  header.writeUInt8(0, e + 2);
  header.writeUInt8(0, e + 3);
  header.writeUInt16LE(1, e + 4);
  header.writeUInt16LE(32, e + 6);
  header.writeUInt32LE(img.length, e + 8);
  header.writeUInt32LE(offset, e + 12);
  offset += img.length;
});
fs.writeFileSync(path.join(pub, 'favicon.ico'), Buffer.concat([header, ...icoImages]));

// apple-touch-icon.png: 180 x 180, white mark on black. The mark is 50 % of the tile.
const touchMark = 90;
const touchSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 180 180"><rect width="180" height="180" fill="#000"/><path fill="#fff" d="${markPath(45, 45, touchMark / SIZE)}"/></svg>`;
fs.writeFileSync(path.join(pub, 'apple-touch-icon.png'), png(touchSvg, 180));

// og.png: 1200 x 630.
fs.writeFileSync(path.join(pub, 'og.png'), await sharp(png(ogSvg(), 1200, '#fff')).png({ compressionLevel: 9, palette: true }).toBuffer());

function ogSvg() {
  const W = 1200, H = 630, M = 80;
  // Langton's ant after the highway forms. Mirrored, so the highway leaves the image at the lower right.
  const cells = langton(11200);
  const cs = 7;
  const xs = cells.map((c) => -c[0]), ys = cells.map((c) => c[1]);
  const minX = Math.min(...xs), minY = Math.min(...ys);
  const gx = 760, gy = 70;
  let antD = '';
  for (const [x0, y] of cells) {
    const x = -x0;
    antD += `M${gx + (x - minX) * cs} ${gy + (y - minY) * cs}h${cs - 1}v${cs - 1}h${-(cs - 1)}z`;
  }
  const wmSize = 44;
  const wmCap = wmSize * (1490 / 2048);
  const wmMarkW = wmCap;
  const wm = textPath('turmite', { size: wmSize, x: M + wmMarkW * 1.34, y: M + wmCap });
  const h1a = textPath('Self-hosted crypto', { size: 68, weight: 650, x: M, y: 318, tracking: -0.035 });
  const h1b = textPath('payment gateway.', { size: 68, weight: 650, x: M, y: 396, tracking: -0.035 });
  const sub = textPath('Free and open source. Runs on your own server.', { size: 28, weight: 450, x: M, y: 466, tracking: -0.01 });
  const url = textPath('turmite.com', { size: 24, weight: 550, x: M, y: H - M + 8, tracking: 0 });
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
<rect width="${W}" height="${H}" fill="#fff"/>
<path fill="#000" d="${markPath(M, M, wmMarkW / SIZE)}"/>
<path fill="#000" d="${wm.d}"/>
<path fill="#000" d="${h1a.d}${h1b.d}"/>
<path fill="#555" d="${sub.d}"/>
<path fill="#000" d="${url.d}"/>
<path fill="#000" d="${antD}"/>
</svg>`;
}

function langton(steps) {
  const black = new Set();
  let x = 0, y = 0, d = 0;
  const dx = [0, 1, 0, -1], dy = [-1, 0, 1, 0];
  for (let s = 0; s < steps; s++) {
    const k = x + ',' + y;
    if (black.has(k)) {
      d = (d + 3) % 4;
      black.delete(k);
    } else {
      d = (d + 1) % 4;
      black.add(k);
    }
    x += dx[d];
    y += dy[d];
  }
  return [...black].map((k) => k.split(',').map(Number));
}

if (previewDir) {
  fs.mkdirSync(previewDir, { recursive: true });
  for (const s of [16, 32, 64, 512]) fs.writeFileSync(path.join(previewDir, `logo-${s}.png`), png(solid(logo, '#000'), s, '#fff'));
  for (const s of [16, 32]) fs.writeFileSync(path.join(previewDir, `favicon-${s}.png`), png(faviconBlack, s, '#fff'));
  fs.writeFileSync(path.join(previewDir, 'wordmark-800.png'), png(solid(wordmark, '#000'), 800, '#fff'));
  fs.writeFileSync(path.join(previewDir, 'wordmark-160.png'), png(solid(wordmark, '#000'), 160, '#fff'));
  fs.writeFileSync(path.join(previewDir, 'wordmark-dark-400.png'), png(solid(wordmark, '#fff'), 400, '#000'));
}
console.log('brand files written to', pub);
