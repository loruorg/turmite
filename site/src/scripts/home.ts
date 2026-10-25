// Home page behaviour. The page reads and works without it.
import type { HeroScene } from './hero3d';
import { startAnt } from './ant2d';

const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

function whenVisible(el: Element, cb: (visible: boolean) => void) {
  let visible = false;
  const update = () => cb(visible && document.visibilityState === 'visible');
  new IntersectionObserver(([e]) => {
    visible = e.isIntersecting;
    update();
  }).observe(el);
  document.addEventListener('visibilitychange', update);
}

function webgl(): boolean {
  try {
    const c = document.createElement('canvas');
    return !!(c.getContext('webgl2') || c.getContext('webgl'));
  } catch {
    return false;
  }
}

// Hero: load three.js only on the client, after the page is idle, and only with motion allowed.
const hero = document.querySelector<HTMLElement>('[data-hero]');
const heroCanvas = hero?.querySelector('canvas');
if (hero && heroCanvas && !reduce && webgl()) {
  let scene: HeroScene | null = null;
  let loading = false;
  let want = false;
  const idle = (cb: () => void) =>
    typeof requestIdleCallback === 'function' ? requestIdleCallback(cb, { timeout: 1500 }) : setTimeout(cb, 300);
  whenVisible(hero, (on) => {
    want = on;
    scene?.setActive(on);
    if (on && !scene && !loading) {
      loading = true;
      idle(async () => {
        try {
          const { createHero } = await import('./hero3d');
          const [ruleEl, stepEl] = hero.querySelectorAll<HTMLElement>('.hero-caption span');
          scene = createHero(
            heroCanvas,
            () => hero.classList.add('is-live'),
            (rule, steps) => {
              ruleEl.textContent = rule === 'RL' ? "Langton's ant" : `Turmite ${rule}`;
              stepEl.textContent = `step ${steps.toLocaleString('en-US')}`;
            },
          );
          scene.setActive(want);
        } catch {
          // The static frame stays.
        }
      });
    }
  });
}

// Community band: 2D ant.
const band = document.querySelector<HTMLCanvasElement>('[data-ant]');
if (band) {
  const ant = startAnt(band, { still: reduce });
  whenVisible(band, (on) => ant.setActive(on));
}

// Copy buttons for commands.
for (const btn of document.querySelectorAll<HTMLButtonElement>('[data-copy]')) {
  btn.hidden = false;
  btn.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(btn.dataset.copy!);
      btn.dataset.done = '';
      window.setTimeout(() => delete btn.dataset.done, 1600);
    } catch {
      // Clipboard not available: the text stays selectable.
    }
  });
}
