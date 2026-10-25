// Hero scene: a turmite builds a voxel structure on a grid. Loaded on demand, after the page is idle.
// It starts as a top-down view that matches the static SVG frame, then the cells extrude and the camera tilts.
import {
  BoxGeometry,
  BufferAttribute,
  Color,
  DynamicDrawUsage,
  EdgesGeometry,
  Group,
  InstancedMesh,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  OrthographicCamera,
  PlaneGeometry,
  Scene,
  ShaderMaterial,
  Vector3,
  WebGLRenderer,
} from 'three';
import { HERO, Turmite, heroTurmite } from '../lib/turmite';

export interface HeroScene {
  setActive(on: boolean): void;
  dispose(): void;
}

const G = HERO.size;
const N = G * G;
const CELL = 0.86;
const RULES = ['LRRRRRLLR', 'RL', 'LLRR', 'RRLLLRLLLRRR'];
const TILT = 0.96; // polar angle of the camera, radians
const AZIMUTH = Math.PI / 4;
const RUN_SECONDS = 28;

const easeInOut = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
const easeOut = (t: number) => 1 - Math.pow(1 - t, 3);
const clamp01 = (t: number) => Math.min(1, Math.max(0, t));
const damp = (k: number, dt: number) => 1 - Math.exp(-k * dt);

function heightFor(color: number, colors: number): number {
  if (color === 0) return 0;
  if (colors === 2) return 1;
  return 0.45 + (1.6 * (color - 1)) / (colors - 2);
}

/** A box with its base at y = 0 and one flat shade per face direction. */
function shadedBox(): BoxGeometry {
  const g = new BoxGeometry(CELL, 1, CELL);
  g.translate(0, 0.5, 0);
  const n = g.getAttribute('normal');
  const colors = new Float32Array(n.count * 3);
  const top = new Color('#0b0b0b');
  const sideX = new Color('#3a3a3a');
  const sideZ = new Color('#626262');
  for (let i = 0; i < n.count; i++) {
    const c = Math.abs(n.getY(i)) > 0.5 ? top : Math.abs(n.getX(i)) > 0.5 ? sideX : sideZ;
    colors.set([c.r, c.g, c.b], i * 3);
  }
  g.setAttribute('color', new BufferAttribute(colors, 3));
  return g;
}

export function createHero(
  canvas: HTMLCanvasElement,
  onReady: () => void,
  onInfo?: (rule: string, steps: number) => void,
): HeroScene {
  const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: 'low-power' });
  renderer.setClearColor(0xffffff, 0);
  const scene = new Scene();
  const camera = new OrthographicCamera(-G / 2, G / 2, G / 2, -G / 2, 0.1, 1000);
  camera.up.set(0, 0, -1);

  // Floor: faint grid lines that fade out toward the edge.
  const floor = new Mesh(
    new PlaneGeometry(G * 1.6, G * 1.6),
    new ShaderMaterial({
      transparent: true,
      depthWrite: false,
      uniforms: { uRadius: { value: G * 0.62 }, uHalf: { value: G / 2 } },
      vertexShader: `varying vec2 vP; void main(){ vec4 w = modelMatrix * vec4(position,1.0); vP = w.xz; gl_Position = projectionMatrix * viewMatrix * w; }`,
      fragmentShader: `varying vec2 vP; uniform float uRadius; uniform float uHalf;
        void main(){
          vec2 g = abs(fract(vP + uHalf + 0.5) - 0.5) / fwidth(vP);
          float line = 1.0 - min(min(g.x, g.y), 1.0);
          float fade = 1.0 - smoothstep(uRadius * 0.55, uRadius, length(vP));
          gl_FragColor = vec4(0.0, 0.0, 0.0, line * 0.075 * fade);
        }`,
    }),
  );
  floor.rotation.x = -Math.PI / 2;
  floor.position.y = -0.02;
  scene.add(floor);

  // Cells: instances only for cells with a height. Slots stay packed at the front.
  const cells = new InstancedMesh(shadedBox(), new MeshBasicMaterial({ vertexColors: true }), N);
  cells.instanceMatrix.setUsage(DynamicDrawUsage);
  cells.frustumCulled = false;
  cells.count = 0;
  scene.add(cells);
  const m = cells.instanceMatrix.array as Float32Array;
  const slotOf = new Int32Array(N).fill(-1);
  const cellAt = new Int32Array(N);
  let count = 0;

  // The turmite: a white block with black edges.
  const antGeo = new BoxGeometry(CELL, 1.3, CELL);
  antGeo.translate(0, 0.65, 0);
  const ant = new Group();
  ant.add(new Mesh(antGeo, new MeshBasicMaterial({ color: 0xffffff })));
  ant.add(new LineSegments(new EdgesGeometry(antGeo), new LineBasicMaterial({ color: 0x000000 })));
  scene.add(ant);

  const heights = new Float32Array(N);
  const targets = new Float32Array(N);
  const moving = new Set<number>();

  let turmite: Turmite = heroTurmite();
  let ruleIndex = 0;
  let phase: 'intro' | 'run' | 'hold' | 'sink' = 'intro';
  let phaseT = 0;
  let stepBudget = 0;
  let speed = 2.4;
  let elapsed = 0;
  let azimuth = 0;
  let polar = 0;
  let zoom = 1;
  const target = new Vector3();
  const goal = new Vector3();
  let goalZoom = 1;
  let minX: number = G;
  let maxX = -1;
  let minY: number = G;
  let maxY = -1;
  let antX = turmite.x;
  let antZ = turmite.y;
  let antH = 0;

  const cx = (i: number) => (i % G) - G / 2 + 0.5;
  const cz = (i: number) => Math.floor(i / G) - G / 2 + 0.5;

  function grow(i: number) {
    const x = i % G;
    const y = (i - x) / G;
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  }

  // Frame the structure: project every filled cell into the resting camera view and fit the extents.
  const probe = new OrthographicCamera(-G / 2, G / 2, G / 2, -G / 2, 0.1, 1000);
  probe.up.set(0, 0, -1);
  const pt = new Vector3();
  function frame3d() {
    if (count === 0) {
      goal.set(0, 0, 0);
      goalZoom = 1.5;
      return;
    }
    const r = 200;
    probe.position.set(r * Math.sin(TILT) * Math.sin(AZIMUTH), r * Math.cos(TILT), r * Math.sin(TILT) * Math.cos(AZIMUTH));
    probe.lookAt(0, 0, 0);
    probe.updateMatrixWorld();
    const inv = probe.matrixWorldInverse;
    let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
    for (let s = 0; s < count; s++) {
      const i = cellAt[s];
      for (const h of [0, targets[i] || heights[i]]) {
        pt.set(cx(i), h, cz(i)).applyMatrix4(inv);
        if (pt.x < x0) x0 = pt.x;
        if (pt.x > x1) x1 = pt.x;
        if (pt.y < y0) y0 = pt.y;
        if (pt.y > y1) y1 = pt.y;
      }
    }
    // Move the target by the offset of the extents' centre, along the camera's right and up axes.
    const mx = (x0 + x1) / 2;
    const my = (y0 + y1) / 2;
    const e = probe.matrixWorld.elements;
    goal.set(e[0] * mx + e[4] * my, e[1] * mx + e[5] * my, e[2] * mx + e[6] * my);
    // The view is G units wide; keep a margin for the turmite, the slow sway and new growth.
    const w = x1 - x0 + 3;
    const h = y1 - y0 + 3;
    goalZoom = Math.min(1.7, Math.max(0.6, (G / Math.max(w, h)) * 0.78));
  }

  function write(i: number, h: number) {
    let s = slotOf[i];
    if (h <= 0.0005) {
      if (s < 0) return;
      // Move the last instance into the freed slot.
      const last = --count;
      const j = cellAt[last];
      if (j !== i) {
        m.copyWithin(s * 16, last * 16, last * 16 + 16);
        cellAt[s] = j;
        slotOf[j] = s;
      }
      slotOf[i] = -1;
      return;
    }
    if (s < 0) {
      s = slotOf[i] = count++;
      cellAt[s] = i;
    }
    const o = s * 16;
    m.fill(0, o, o + 16);
    m[o] = 1;
    m[o + 5] = h;
    m[o + 10] = 1;
    m[o + 12] = cx(i);
    m[o + 14] = cz(i);
    m[o + 15] = 1;
  }

  for (let i = 0; i < N; i++) {
    targets[i] = heightFor(turmite.grid[i], turmite.colors);
    heights[i] = targets[i] ? 0.001 : 0;
    if (targets[i]) grow(i);
    write(i, heights[i]);
  }
  cells.count = count;
  cells.instanceMatrix.needsUpdate = true;
  frame3d();

  function resize() {
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    if (!w || !h) return;
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(w, h, false);
    const aspect = w / h;
    const half = G / 2;
    camera.left = -half * Math.max(1, aspect);
    camera.right = half * Math.max(1, aspect);
    camera.top = half * Math.max(1, 1 / aspect);
    camera.bottom = -half * Math.max(1, 1 / aspect);
    camera.updateProjectionMatrix();
  }
  const ro = new ResizeObserver(resize);
  ro.observe(canvas);
  resize();

  function placeCamera() {
    const r = 200;
    camera.position.set(
      target.x + r * Math.sin(polar) * Math.sin(azimuth),
      target.y + r * Math.cos(polar),
      target.z + r * Math.sin(polar) * Math.cos(azimuth),
    );
    camera.lookAt(target);
    camera.zoom = zoom;
    camera.updateProjectionMatrix();
  }

  function startRule() {
    const rule = RULES[ruleIndex++ % RULES.length];
    turmite = new Turmite(G, rule, G >> 1, G >> 1, 0);
    targets.fill(0);
    minX = minY = G;
    maxX = maxY = -1;
    speed = rule === 'RL' ? 16 : 10;
    antX = turmite.x;
    antZ = turmite.y;
    frame3d();
  }

  function stepSim(n: number) {
    for (let k = 0; k < n; k++) {
      const i = turmite.step();
      if (i < 0) return;
      targets[i] = heightFor(turmite.grid[i], turmite.colors);
      if (targets[i]) grow(i);
      moving.add(i);
    }
  }

  let raf = 0;
  let frameNo = 0;
  let last = 0;
  let active = false;
  let disposed = false;
  let ready = false;

  function frame(now: number) {
    raf = requestAnimationFrame(frame);
    const dt = Math.min(0.05, last ? (now - last) / 1000 : 0.016);
    last = now;
    elapsed += dt;
    phaseT += dt;
    let dirty = false;

    if (phase === 'intro') {
      // Extrude from the turmite outward while the camera tilts and frames the structure.
      const t = phaseT;
      const cam = easeInOut(clamp01((t - 0.35) / 2.4));
      polar = cam * TILT;
      azimuth = cam * AZIMUTH;
      zoom = 1 + (goalZoom - 1) * cam;
      target.copy(goal).multiplyScalar(cam);
      for (let s = 0; s < count; s++) {
        const i = cellAt[s];
        const d = Math.hypot((i % G) - turmite.x, Math.floor(i / G) - turmite.y) / G;
        const p = easeOut(clamp01((t - 0.5 - d * 1.4) / 0.9));
        heights[i] = Math.max(0.001, targets[i] * p);
        write(i, heights[i]);
      }
      dirty = true;
      if (t > 3) {
        phase = 'run';
        phaseT = 0;
      }
    } else if (phase === 'run') {
      stepBudget += speed * dt * 60 * Math.min(1, phaseT / 2 + 0.15);
      const n = Math.floor(stepBudget);
      stepBudget -= n;
      stepSim(n);
      if (!turmite.alive || phaseT > RUN_SECONDS) {
        phase = 'hold';
        phaseT = 0;
      }
    } else if (phase === 'hold') {
      if (phaseT > 2.4) {
        phase = 'sink';
        phaseT = 0;
        for (let s = 0; s < count; s++) moving.add(cellAt[s]);
      }
    } else if (phase === 'sink') {
      // A diagonal wave lowers every cell, then a new turmite starts in the middle.
      for (const i of moving) {
        const d = ((i % G) + Math.floor(i / G)) / (2 * G);
        const p = clamp01((phaseT - d * 1.2) / 0.5);
        heights[i] = targets[i] = heights[i] * (1 - p * 0.35);
        if (p >= 1 || heights[i] < 0.01) {
          heights[i] = targets[i] = 0;
          moving.delete(i);
        }
        write(i, heights[i]);
      }
      dirty = true;
      if (!moving.size) {
        startRule();
        phase = 'run';
        phaseT = 0;
      }
    }

    if (phase === 'run' || phase === 'hold') {
      const k = damp(14, dt);
      for (const i of moving) {
        const h = heights[i] + (targets[i] - heights[i]) * k;
        if (Math.abs(targets[i] - h) < 0.002) {
          heights[i] = targets[i];
          moving.delete(i);
        } else heights[i] = h;
        write(i, heights[i]);
      }
      dirty = true;
    }

    if (phase !== 'intro') {
      if ((frameNo & 15) === 0) frame3d();
      azimuth = AZIMUTH + Math.sin(elapsed * 0.11) * 0.22;
      polar = TILT;
      const k = damp(1.2, dt);
      zoom += (goalZoom - zoom) * k;
      target.lerp(goal, k);
    }

    // Turmite marker follows the turmite, on top of its cell.
    const kx = damp(22, dt);
    antX += (turmite.x - antX) * kx;
    antZ += (turmite.y - antZ) * kx;
    const ti = Math.min(G - 1, Math.max(0, turmite.y)) * G + Math.min(G - 1, Math.max(0, turmite.x));
    antH += (heights[ti] - antH) * kx;
    ant.position.set(antX - G / 2 + 0.5, antH, antZ - G / 2 + 0.5);
    ant.visible = turmite.alive && phase !== 'sink' && (phase !== 'intro' || phaseT > 0.6);
    ant.scale.y = phase === 'intro' ? Math.max(0.001, clamp01((phaseT - 0.6) / 0.6)) : 1;

    if (onInfo && (frameNo & 7) === 0) onInfo(turmite.rule, turmite.steps);
    frameNo++;
    if (dirty) {
      cells.count = count;
      cells.instanceMatrix.needsUpdate = true;
    }
    placeCamera();
    renderer.render(scene, camera);
    if (!ready) {
      ready = true;
      onReady();
    }
  }

  function setActive(on: boolean) {
    if (disposed || on === active) return;
    active = on;
    if (on) {
      last = 0;
      raf = requestAnimationFrame(frame);
    } else cancelAnimationFrame(raf);
  }

  function dispose() {
    disposed = true;
    cancelAnimationFrame(raf);
    ro.disconnect();
    renderer.dispose();
  }

  placeCamera();
  return { setActive, dispose };
}
