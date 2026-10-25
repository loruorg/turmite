import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { docsOrder } from '../lib/docs-nav';
import { apiMarkdown, loadApi } from '../lib/openapi';
import { stripVerifyText } from '../lib/markdown-plugins.mjs';

export const GET: APIRoute = async ({ site }) => {
  const base = (site ?? new URL('https://turmite.com')).href.replace(/\/$/, '');
  const entries = (await getCollection('docs'))
    .filter((e) => docsOrder.includes(e.id))
    .sort((a, b) => docsOrder.indexOf(a.id) - docsOrder.indexOf(b.id));
  let out = `# Turmite documentation\n\nEvery page of ${base}/docs/ in one file. Index: ${base}/llms.txt\n`;
  for (const e of entries) {
    const url = e.id === 'index' ? `${base}/docs/` : `${base}/docs/${e.id}/`;
    out += `\n\n# ${e.data.title}\n\n> ${e.data.description}\n\nSource: ${url}\n\n${stripVerifyText(e.body ?? '').trim()}\n`;
    if (e.id === 'api-reference') out += apiMarkdown(loadApi());
  }
  return new Response(out, { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
};
