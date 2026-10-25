// Reads ../openapi.yaml (repository root) at build time and turns it into a simple model for the
// API reference page. The build works when the file does not exist yet.
import fs from 'node:fs';
import path from 'node:path';
import { parse } from 'yaml';

type Json = any;

export interface Field {
  name: string;
  type: string;
  required: boolean;
  description: string;
  depth: number;
  values?: string[];
}

export interface Param {
  name: string;
  in: string;
  type: string;
  required: boolean;
  description: string;
  values?: string[];
}

export interface Response {
  code: string;
  description: string;
  example?: string;
}

export interface Operation {
  id: string;
  method: string;
  path: string;
  summary: string;
  description: string;
  auth: string[];
  params: Param[];
  body: Field[];
  bodyExample?: string;
  responses: Response[];
}

export interface Group {
  name: string;
  slug: string;
  description: string;
  ops: Operation[];
}

export interface ApiModel {
  title: string;
  version: string;
  description: string;
  groups: Group[];
}

export const OPENAPI_FILE = path.resolve(process.cwd(), '..', 'openapi.yaml');

let cached: ApiModel | null | undefined;

export function loadApi(): ApiModel | null {
  if (cached !== undefined) return cached;
  if (!fs.existsSync(OPENAPI_FILE)) return (cached = null);
  try {
    cached = build(parse(fs.readFileSync(OPENAPI_FILE, 'utf8')));
  } catch (e) {
    console.warn(`[openapi] cannot read ${OPENAPI_FILE}: ${(e as Error).message}`);
    cached = null;
  }
  return cached;
}

export function slugify(s: string): string {
  return s
    .toLowerCase()
    .replace(/[{}]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');
}

function build(doc: Json): ApiModel {
  const resolve = (node: Json, seen = new Set<string>()): Json => {
    if (!node || typeof node !== 'object') return node;
    if (typeof node.$ref === 'string' && node.$ref.startsWith('#/')) {
      if (seen.has(node.$ref)) return {};
      const target = node.$ref
        .slice(2)
        .split('/')
        .reduce((o: Json, k: string) => o?.[k.replace(/~1/g, '/').replace(/~0/g, '~')], doc);
      const { $ref, ...rest } = node;
      return { ...resolve(target, new Set([...seen, node.$ref])), ...rest };
    }
    return node;
  };

  const typeOf = (s: Json): string => {
    s = resolve(s);
    if (!s) return 'any';
    if (s.enum) return s.type ? String(Array.isArray(s.type) ? s.type[0] : s.type) : 'string';
    if (s.oneOf || s.anyOf) return (s.oneOf ?? s.anyOf).map(typeOf).join(' | ');
    let t = Array.isArray(s.type) ? s.type.filter((x: string) => x !== 'null').join(' | ') : s.type;
    if (t === 'array') return `${typeOf(s.items)}[]`;
    if (!t && s.properties) t = 'object';
    if (!t) t = 'any';
    if (s.format) t += ` (${s.format})`;
    if (Array.isArray(s.type) && s.type.includes('null')) t += ' | null';
    if (s.nullable) t += ' | null';
    return t;
  };

  const enumOf = (sc: Json): string[] | undefined => {
    const r = merge(sc);
    const e = r?.enum ?? (r?.type === 'array' ? merge(r.items)?.enum : undefined);
    return e ? e.map((v: Json) => String(v)) : undefined;
  };

  const merge = (s: Json): Json => {
    s = resolve(s);
    if (s?.allOf) {
      const parts = s.allOf.map(merge);
      return {
        ...s,
        type: 'object',
        properties: Object.assign({}, ...parts.map((p: Json) => p.properties ?? {}), s.properties ?? {}),
        required: [...new Set([...parts.flatMap((p: Json) => p.required ?? []), ...(s.required ?? [])])],
      };
    }
    return s;
  };

  const fields = (schema: Json, prefix = '', depth = 0, out: Field[] = []): Field[] => {
    const s = merge(schema);
    if (!s || depth > 3) return out;
    const target = s.type === 'array' ? merge(s.items) : s;
    const props = target?.properties ?? {};
    const req: string[] = target?.required ?? [];
    for (const [name, raw] of Object.entries<Json>(props)) {
      const p = merge(raw);
      const full = prefix ? `${prefix}.${name}` : name;
      out.push({
        name: full,
        type: typeOf(p),
        required: req.includes(name),
        description: (p.description ?? '').trim(),
        depth,
        values: enumOf(p),
      });
      const inner = p.type === 'array' ? merge(p.items) : p;
      if (inner?.properties) fields(inner, p.type === 'array' ? `${full}[]` : full, depth + 1, out);
    }
    return out;
  };

  const example = (schema: Json, depth = 0): Json => {
    const s = merge(schema);
    if (!s || depth > 5) return null;
    if (s.example !== undefined) return s.example;
    if (s.examples?.length) return s.examples[0];
    if (s.default !== undefined) return s.default;
    if (s.enum) return s.enum[0];
    if (s.oneOf || s.anyOf) return example((s.oneOf ?? s.anyOf)[0], depth + 1);
    const t = Array.isArray(s.type) ? s.type.find((x: string) => x !== 'null') : s.type;
    if (t === 'array') return [example(s.items, depth + 1)];
    if (t === 'object' || s.properties) {
      const o: Record<string, Json> = {};
      for (const [k, v] of Object.entries<Json>(s.properties ?? {})) o[k] = example(v, depth + 1);
      return o;
    }
    if (t === 'integer' || t === 'number') return 0;
    if (t === 'boolean') return false;
    if (t === 'string') return s.format === 'date-time' ? '2026-10-23T12:00:00Z' : 'string';
    return null;
  };

  const mediaExample = (content: Json): string | undefined => {
    const media = content?.['application/json'] ?? Object.values<Json>(content ?? {})[0];
    if (!media) return undefined;
    let ex = media.example;
    if (ex === undefined && media.examples) ex = resolve(Object.values<Json>(media.examples)[0])?.value;
    if (ex === undefined && media.schema) ex = example(media.schema);
    return ex === undefined ? undefined : JSON.stringify(ex, null, 2);
  };

  const securityNames = (sec: Json[] | undefined): string[] => {
    const list = sec ?? doc.security ?? [];
    return [...new Set(list.flatMap((r: Json) => Object.keys(r)))] as string[];
  };

  const operation = (p: string, method: string, op: Json, shared: Json[]): Operation => {
    const params: Param[] = [...shared, ...(op.parameters ?? [])].map((x: Json) => resolve(x)).map((x: Json) => ({
      name: x.name,
      in: x.in,
      type: typeOf(x.schema),
      required: !!x.required,
      description: (x.description ?? '').trim(),
      values: enumOf(x.schema),
    }));
    const body = resolve(op.requestBody);
    const bodySchema = body?.content?.['application/json']?.schema;
    return {
      id: op.operationId ?? slugify(`${method} ${p}`),
      method: method.toUpperCase(),
      path: p,
      summary: op.summary ?? '',
      description: (op.description ?? '').trim(),
      auth: securityNames(op.security),
      params,
      body: bodySchema ? fields(bodySchema) : [],
      bodyExample: body ? mediaExample(body.content) : undefined,
      responses: Object.entries<Json>(op.responses ?? {}).map(([code, r]) => {
        const res = resolve(r);
        return { code, description: (res?.description ?? '').trim(), example: mediaExample(res?.content) };
      }),
    };
  };

  const methods = ['get', 'post', 'put', 'patch', 'delete'];
  const tagMeta = new Map<string, string>((doc.tags ?? []).map((t: Json) => [t.name, t.description ?? '']));
  const groups = new Map<string, Group>();
  const group = (tag: string): Group => {
    if (!groups.has(tag)) groups.set(tag, { name: tag, slug: slugify(tag), description: tagMeta.get(tag) ?? '', ops: [] });
    return groups.get(tag)!;
  };
  const add = (tag: string, op: Operation) => group(tag).ops.push(op);
  // Keep the order of the document's tag list.
  for (const t of doc.tags ?? []) group(t.name);

  for (const [p, item] of Object.entries<Json>(doc.paths ?? {})) {
    for (const m of methods) {
      if (!item?.[m]) continue;
      add(item[m].tags?.[0] ?? 'Endpoints', operation(p, m, item[m], item.parameters ?? []));
    }
  }
  for (const [name, item] of Object.entries<Json>(doc.webhooks ?? {})) {
    for (const m of methods) {
      if (!item?.[m]) continue;
      add('Webhooks', operation(name, m, item[m], item.parameters ?? []));
    }
  }

  return {
    title: doc.info?.title ?? 'API',
    version: doc.info?.version ?? '',
    description: (doc.info?.description ?? '').trim(),
    groups: [...groups.values()].filter((g) => g.ops.length),
  };
}

/** Markdown version of the reference, for the .md page and llms-full.txt. */
export function apiMarkdown(api: ApiModel | null): string {
  if (!api) return '';
  const esc = (s: string) => s.replace(/\|/g, '\\|').replace(/\n+/g, ' ');
  let md = '';
  for (const g of api.groups) {
    md += `\n## ${g.name}\n\n`;
    if (g.description) md += `${g.description}\n\n`;
    for (const op of g.ops) {
      md += `### ${op.method} ${op.path}\n\n`;
      if (op.summary) md += `${op.summary}\n\n`;
      if (op.description) md += `${op.description}\n\n`;
      if (op.auth.length) md += `Auth: ${op.auth.map((a) => `\`${a}\``).join(', ')}\n\n`;
      if (op.params.length) {
        md += `| Parameter | In | Type | Required | Description |\n|---|---|---|---|---|\n`;
        for (const p of op.params)
          md += `| \`${p.name}\` | ${p.in} | \`${esc(p.type)}\` | ${p.required ? 'yes' : 'no'} | ${esc(p.description)}${p.values ? ` One of: ${p.values.map((v) => `\`${v}\``).join(', ')}.` : ''} |\n`;
        md += '\n';
      }
      if (op.body.length) {
        md += `| Field | Type | Required | Description |\n|---|---|---|---|\n`;
        for (const f of op.body)
          md += `| \`${f.name}\` | \`${esc(f.type)}\` | ${f.required ? 'yes' : 'no'} | ${esc(f.description)}${f.values ? ` One of: ${f.values.map((v) => `\`${v}\``).join(', ')}.` : ''} |\n`;
        md += '\n';
      }
      if (op.bodyExample) md += '```json\n' + op.bodyExample + '\n```\n\n';
      for (const r of op.responses) {
        md += `Response \`${r.code}\`: ${r.description}\n\n`;
        if (r.example) md += '```json\n' + r.example + '\n```\n\n';
      }
    }
  }
  return md;
}
