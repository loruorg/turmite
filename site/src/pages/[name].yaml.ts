import fs from 'node:fs';
import type { APIRoute } from 'astro';
import { OPENAPI_FILE } from '../lib/openapi';

// Serves /openapi.yaml when the repository has one.
export function getStaticPaths() {
  return fs.existsSync(OPENAPI_FILE) ? [{ params: { name: 'openapi' } }] : [];
}

export const GET: APIRoute = () =>
  new Response(fs.readFileSync(OPENAPI_FILE, 'utf8'), { headers: { 'Content-Type': 'application/yaml; charset=utf-8' } });
