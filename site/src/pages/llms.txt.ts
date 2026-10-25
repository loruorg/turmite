import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { docsNav } from '../lib/docs-nav';

export const GET: APIRoute = async ({ site }) => {
  const base = (site ?? new URL('https://turmite.com')).href.replace(/\/$/, '');
  const entries = new Map((await getCollection('docs')).map((e) => [e.id, e]));
  let out = `# Turmite

> Turmite is a free, MIT-licensed, self-hosted crypto payment gateway. One Rust binary and PostgreSQL. Networks: Bitcoin, Monero, Ethereum, Base, Polygon, BNB Smart Chain, Tron, Solana and TON, with USDC and USDT where listed. The merchant API uses the same paths, fields, statuses and IPN signature as the common hosted-gateway API, so an existing integration changes only the base URL, the API key and the IPN secret. There is no hosted service, no fee and no account.

Install on a Linux server: \`curl -fsSL ${base}/install.sh | sudo sh -s -- --site-url https://pay.example.com\`. Source: https://github.com/loruorg/turmite. Image: ghcr.io/loruorg/turmite.

Each docs page is also available as Markdown at the URL below. All pages in one file: ${base}/llms-full.txt
`;
  for (const s of docsNav) {
    out += `\n## ${s.section}\n\n`;
    for (const slug of s.items) {
      const e = entries.get(slug);
      if (!e) continue;
      out += `- [${e.data.title}](${base}/docs/${slug}.md): ${e.data.description}\n`;
    }
  }
  return new Response(out, { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
};
