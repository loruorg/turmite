/** Raw webhook body as received. */
export type RawBody = string | Uint8Array | ArrayBuffer;

type Node =
  | { k: 'object'; members: Array<[string, Node]> }
  | { k: 'array'; items: Node[] }
  | { k: 'string'; value: string }
  | { k: 'literal'; text: string };

const MAX_DEPTH = 512;
const NUMBER = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/y;
const LITERAL = /true|false|null/y;

class Parser {
  private i = 0;

  constructor(private readonly s: string) {}

  parse(): Node {
    const node = this.value(0);
    this.ws();
    if (this.i !== this.s.length) this.fail();
    return node;
  }

  private fail(): never {
    throw new SyntaxError(`Invalid JSON at position ${this.i}`);
  }

  private ws(): void {
    for (;;) {
      const c = this.s.charCodeAt(this.i);
      if (c !== 0x20 && c !== 0x09 && c !== 0x0a && c !== 0x0d) return;
      this.i++;
    }
  }

  private value(depth: number): Node {
    if (depth > MAX_DEPTH) this.fail();
    this.ws();
    switch (this.s[this.i]) {
      case '{':
        return this.object(depth + 1);
      case '[':
        return this.array(depth + 1);
      case '"':
        return { k: 'string', value: this.string() };
      default:
        return { k: 'literal', text: this.literal() };
    }
  }

  private object(depth: number): Node {
    const members: Array<[string, Node]> = [];
    this.i++;
    this.ws();
    if (this.s[this.i] === '}') {
      this.i++;
      return { k: 'object', members };
    }
    for (;;) {
      this.ws();
      if (this.s[this.i] !== '"') this.fail();
      const key = this.string();
      this.ws();
      if (this.s[this.i] !== ':') this.fail();
      this.i++;
      members.push([key, this.value(depth)]);
      this.ws();
      const c = this.s[this.i++];
      if (c === '}') return { k: 'object', members };
      if (c !== ',') this.fail();
    }
  }

  private array(depth: number): Node {
    const items: Node[] = [];
    this.i++;
    this.ws();
    if (this.s[this.i] === ']') {
      this.i++;
      return { k: 'array', items };
    }
    for (;;) {
      items.push(this.value(depth));
      this.ws();
      const c = this.s[this.i++];
      if (c === ']') return { k: 'array', items };
      if (c !== ',') this.fail();
    }
  }

  private string(): string {
    const start = this.i;
    let j = start + 1;
    for (;;) {
      const c = this.s.charCodeAt(j);
      if (Number.isNaN(c)) this.fail();
      if (c === 0x22) break;
      j += c === 0x5c ? 2 : 1;
    }
    this.i = j + 1;
    // JSON.parse decodes the escapes and rejects raw control characters.
    return JSON.parse(this.s.slice(start, j + 1)) as string;
  }

  private literal(): string {
    for (const re of [NUMBER, LITERAL]) {
      re.lastIndex = this.i;
      const m = re.exec(this.s);
      if (m) {
        this.i += m[0].length;
        return m[0];
      }
    }
    this.fail();
  }
}

/** Orders UTF-16 strings like their UTF-8 bytes (code point order). */
export function compareKeys(a: string, b: string): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    let x = a.charCodeAt(i);
    let y = b.charCodeAt(i);
    if (x !== y) {
      if (x >= 0xd800) x += x <= 0xdfff ? 0x2000 : -0x800;
      if (y >= 0xd800) y += y <= 0xdfff ? 0x2000 : -0x800;
      return x - y;
    }
  }
  return a.length - b.length;
}

function write(node: Node): string {
  switch (node.k) {
    case 'object':
      return `{${node.members
        .sort((a, b) => compareKeys(a[0], b[0]))
        .map(([key, value]) => `${JSON.stringify(key)}:${write(value)}`)
        .join(',')}}`;
    case 'array':
      return `[${node.items.map(write).join(',')}]`;
    case 'string':
      return JSON.stringify(node.value);
    case 'literal':
      return node.text;
  }
}

export function isRawBody(input: unknown): input is RawBody {
  return (
    typeof input === 'string' ||
    ArrayBuffer.isView(input) ||
    Object.prototype.toString.call(input) === '[object ArrayBuffer]'
  );
}

export function bodyText(raw: RawBody): string {
  return typeof raw === 'string' ? raw : new TextDecoder().decode(raw);
}

/**
 * The key-sorted JSON that the IPN signature covers: object keys sorted by
 * UTF-8 bytes at every level, no whitespace, numbers copied as sent.
 *
 * Give the raw body. A parsed object works only when its numbers print the
 * same in JavaScript as in the body (`100.0` in the body prints as `100`).
 */
export function canonicalJson(input: RawBody | object): string {
  let text: string | undefined;
  if (isRawBody(input)) {
    text = bodyText(input);
  } else {
    text = JSON.stringify(input) as string | undefined;
    if (text === undefined) throw new TypeError('Input is not JSON serializable');
  }
  return write(new Parser(text).parse());
}
