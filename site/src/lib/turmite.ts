// A turmite on a square grid. Rule letters: R = turn right, L = turn left, one letter per cell colour.
// On a cell of colour c: turn by rule[c], set the colour to (c + 1) % n, move one cell forward.
const DX = [0, 1, 0, -1];
const DY = [-1, 0, 1, 0];

export class Turmite {
  readonly size: number;
  readonly rule: string;
  readonly colors: number;
  grid: Uint8Array;
  x: number;
  y: number;
  dir: number;
  steps = 0;
  alive = true;

  constructor(size: number, rule = 'RL', x = size >> 1, y = size >> 1, dir = 0) {
    this.size = size;
    this.rule = rule;
    this.colors = rule.length;
    this.grid = new Uint8Array(size * size);
    this.x = x;
    this.y = y;
    this.dir = dir;
  }

  /** One step. Returns the index of the changed cell, or -1 when the turmite has left the grid. */
  step(): number {
    if (!this.alive) return -1;
    const i = this.y * this.size + this.x;
    const c = this.grid[i];
    this.dir = (this.dir + (this.rule.charCodeAt(c) === 82 ? 1 : 3)) & 3;
    this.grid[i] = (c + 1) % this.colors;
    this.x += DX[this.dir];
    this.y += DY[this.dir];
    this.steps++;
    if (this.x < 0 || this.y < 0 || this.x >= this.size || this.y >= this.size) this.alive = false;
    return i;
  }

  run(n: number): void {
    for (let k = 0; k < n && this.alive; k++) this.step();
  }
}

/** The hero starts from this state: Langton's ant after the highway has formed. */
export const HERO = { size: 80, rule: 'RL', x: 50, y: 24, dir: 0, steps: 10900 } as const;

export function heroTurmite(): Turmite {
  const t = new Turmite(HERO.size, HERO.rule, HERO.x, HERO.y, HERO.dir);
  t.run(HERO.steps);
  return t;
}

/** SVG path for the coloured cells, one unit per cell, inset by `gap`. */
export function cellsPath(t: Turmite, gap = 0.08): string {
  const s = 1 - gap;
  const o = gap / 2;
  let d = '';
  for (let y = 0; y < t.size; y++) {
    for (let x = 0; x < t.size; x++) {
      if (t.grid[y * t.size + x]) d += `M${+(x + o).toFixed(2)} ${+(y + o).toFixed(2)}h${s}v${s}h-${s}z`;
    }
  }
  return d;
}
