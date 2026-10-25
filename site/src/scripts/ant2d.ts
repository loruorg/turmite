// Langton's ant on a 2D canvas: light cells on a dark band. No dependencies.
const DX = [0, 1, 0, -1];
const DY = [-1, 0, 1, 0];

export function startAnt(canvas: HTMLCanvasElement, opts: { cell?: number; color?: string; still?: boolean } = {}) {
  const ctx = canvas.getContext('2d');
  if (!ctx) return { setActive() {} };
  const cell = opts.cell ?? 6;
  const color = opts.color ?? 'rgba(255,255,255,0.16)';
  let cols = 0;
  let rows = 0;
  let grid = new Uint8Array(0);
  let x = 0;
  let y = 0;
  let d = 0;
  let steps = 0;
  let raf = 0;
  let active = false;
  let fade = 0;

  function reset() {
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
    cols = Math.ceil(w / cell);
    rows = Math.ceil(h / cell);
    grid = new Uint8Array(cols * rows);
    x = Math.round(cols * 0.62);
    y = Math.round(rows * 0.42);
    d = 0;
    steps = 0;
    fade = 0;
    ctx!.clearRect(0, 0, w, h);
  }

  function paint(i: number) {
    const cx = (i % cols) * cell;
    const cy = Math.floor(i / cols) * cell;
    ctx!.clearRect(cx, cy, cell, cell);
    if (grid[i]) {
      ctx!.fillStyle = color;
      ctx!.fillRect(cx + 1, cy + 1, cell - 1, cell - 1);
    }
  }

  function step(): boolean {
    if (x < 0 || y < 0 || x >= cols || y >= rows) return false;
    const i = y * cols + x;
    d = (d + (grid[i] ? 3 : 1)) & 3;
    grid[i] ^= 1;
    paint(i);
    x += DX[d];
    y += DY[d];
    steps++;
    return true;
  }

  function frame() {
    raf = requestAnimationFrame(frame);
    if (fade > 0) {
      fade -= 1;
      canvas.style.opacity = String(fade / 60);
      if (fade === 0) {
        reset();
        canvas.style.opacity = '1';
      }
      return;
    }
    for (let k = 0; k < 24; k++) {
      if (!step()) {
        fade = 60;
        break;
      }
    }
  }

  reset();
  if (opts.still) {
    while (steps < 11000 && step());
  }
  new ResizeObserver(() => {
    if (canvas.width !== Math.round(canvas.clientWidth * Math.min(window.devicePixelRatio || 1, 2))) reset();
  }).observe(canvas);

  return {
    setActive(on: boolean) {
      if (opts.still || on === active) return;
      active = on;
      if (on) raf = requestAnimationFrame(frame);
      else cancelAnimationFrame(raf);
    },
  };
}
