import fs from 'node:fs';
import path from 'node:path';
import type { APIRoute } from 'astro';

// Serves /install.sh: deploy/install.sh of the repository, read at build time.
const INSTALL_FILE = path.resolve(process.cwd(), '..', 'deploy', 'install.sh');

export const GET: APIRoute = () =>
  new Response(fs.readFileSync(INSTALL_FILE, 'utf8'), { headers: { 'Content-Type': 'text/plain; charset=utf-8' } });
