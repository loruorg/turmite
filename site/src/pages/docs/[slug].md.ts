import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';
import { docsOrder } from '../../lib/docs-nav';
import { apiMarkdown, loadApi } from '../../lib/openapi';
import { stripVerifyText } from '../../lib/markdown-plugins.mjs';

export async function getStaticPaths() {
  const entries = await getCollection('docs');
  return entries.filter((e) => docsOrder.includes(e.id)).map((entry) => ({ params: { slug: entry.id }, props: { entry } }));
}

export const GET: APIRoute = ({ props }) => {
  const { entry } = props as { entry: Awaited<ReturnType<typeof getCollection<'docs'>>>[number] };
  let body = `# ${entry.data.title}\n\n> ${entry.data.description}\n\n${stripVerifyText(entry.body ?? '').trim()}\n`;
  if (entry.id === 'api-reference') body += apiMarkdown(loadApi());
  return new Response(body, { headers: { 'Content-Type': 'text/markdown; charset=utf-8' } });
};
