// Screenshots for visual review. Start `pnpm preview` first.
// Usage: node scripts/shots.mjs [base URL] [output dir] [filter]
import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

const base = process.argv[2] ?? 'http://localhost:4321';
const out = process.argv[3] ?? '/tmp/turmite-shots';
const filter = process.argv[4] ?? '';
fs.mkdirSync(out, { recursive: true });

const pages = [
  { name: 'home', url: '/', wait: 5000 },
  { name: 'docs', url: '/docs/' },
  { name: 'webhooks', url: '/docs/webhooks/' },
  { name: 'api', url: '/docs/api-reference/' },
  { name: 'blog', url: '/blog/' },
  { name: 'post', url: '/blog/turmite-0-1-0/' },
  { name: '404', url: '/nope/' },
];
const widths = [
  { w: 1440, h: 900 },
  { w: 390, h: 844 },
];

// The full Chromium build in headless mode, with the GPU, so the WebGL hero runs at full speed.
const browser = await chromium.launch({ channel: 'chromium', args: ['--use-angle=metal', '--enable-gpu', '--ignore-gpu-blocklist'] });
const shots = [];

async function shoot(name, url, { width, height, js = true, scheme = 'light', full = true, wait = 600, scrolls = [] }) {
  const ctx = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: 1,
    javaScriptEnabled: js,
    colorScheme: scheme,
  });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(base + url, { waitUntil: 'networkidle' });
  await page.waitForTimeout(wait);
  const file = path.join(out, `${name}.png`);
  await page.screenshot({ path: file, fullPage: full });
  shots.push(file);
  for (const y of scrolls) {
    await page.evaluate((top) => window.scrollTo({ top, behavior: 'instant' }), y);
    await page.waitForTimeout(500);
    const f = path.join(out, `${name}-at${y}.png`);
    await page.screenshot({ path: f });
    shots.push(f);
  }
  if (errors.length) console.log(`${name}: ${errors.join(' | ')}`);
  await ctx.close();
}

for (const p of pages) {
  for (const v of widths) {
    const name = `${p.name}-${v.w}`;
    if (filter && !name.includes(filter)) continue;
    await shoot(name, p.url, { width: v.w, height: v.h, wait: p.wait });
  }
}
if (!filter || 'nojs'.includes(filter) || filter === 'home') {
  for (const v of widths) await shoot(`home-nojs-${v.w}`, '/', { width: v.w, height: v.h, js: false });
}
if (!filter || filter === 'dark') {
  await shoot('webhooks-dark-1440', '/docs/webhooks/', { width: 1440, height: 900, scheme: 'dark', full: false });
  await shoot('docs-dark-390', '/docs/', { width: 390, height: 844, scheme: 'dark', full: false });
}
if (filter === 'hero') {
  await shoot('hero-1440', '/', { width: 1440, height: 900, full: false, wait: 7000 });
  await shoot('hero-390', '/', { width: 390, height: 844, full: false, wait: 7000 });
}
if (filter === 'scroll') {
  const ys = (process.argv[5] ?? '900,1500,2100,2700,3300').split(',').map(Number);
  await shoot('scroll-1440', '/', { width: 1440, height: 900, full: false, wait: 1000, scrolls: ys });
  if (process.argv[6]) await shoot('scroll-390', '/', { width: 390, height: 844, full: false, wait: 1000, scrolls: process.argv[6].split(',').map(Number) });
}
await browser.close();
console.log(shots.join('\n'));
