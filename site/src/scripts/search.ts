// Docs search with the static Pagefind index (built after `astro build`). No external service.

interface SubResult {
  title: string;
  url: string;
  excerpt: string;
}
interface ResultData {
  url: string;
  meta: { title?: string };
  excerpt: string;
  sub_results?: SubResult[];
}
interface Pagefind {
  options(o: Record<string, unknown>): Promise<void>;
  debouncedSearch(q: string, o?: unknown, ms?: number): Promise<{ results: { data(): Promise<ResultData> }[] } | null>;
}

const dialog = document.querySelector<HTMLDialogElement>('[data-search]');
const trigger = document.querySelector<HTMLButtonElement>('[data-search-open]');

if (dialog && trigger) {
  const input = dialog.querySelector<HTMLInputElement>('input')!;
  const list = dialog.querySelector<HTMLOListElement>('.results')!;
  const status = dialog.querySelector<HTMLElement>('.status')!;
  let pagefind: Pagefind | null = null;
  let active = -1;

  trigger.hidden = false;

  async function load(): Promise<Pagefind | null> {
    if (pagefind) return pagefind;
    try {
      const url = '/pagefind/pagefind.js';
      pagefind = (await import(/* @vite-ignore */ url)) as Pagefind;
      await pagefind.options({ excerptLength: 22 });
    } catch {
      status.textContent = 'Search is not available in this build.';
    }
    return pagefind;
  }

  function open() {
    if (!dialog!.open) dialog!.showModal();
    input.select();
    void load();
  }

  function setActive(i: number) {
    const items = [...list.querySelectorAll<HTMLAnchorElement>('a')];
    if (!items.length) return;
    active = (i + items.length) % items.length;
    items.forEach((a, k) => a.setAttribute('aria-selected', String(k === active)));
    items[active].scrollIntoView({ block: 'nearest' });
  }

  function clean(url: string): string {
    return url.replace(/\/index\.html$/, '/').replace(/\.html(#|$)/, '/$1');
  }

  async function run() {
    const q = input.value.trim();
    const pf = await load();
    if (!pf) return;
    if (!q) {
      list.innerHTML = '';
      status.textContent = '';
      return;
    }
    const search = await pf.debouncedSearch(q, {}, 120);
    if (!search) return;
    const data = await Promise.all(search.results.slice(0, 8).map((r) => r.data()));
    if (input.value.trim() !== q) return;
    active = -1;
    status.textContent = data.length ? '' : `No results for "${q}".`;
    list.innerHTML = data
      .map((d) => {
        const subs = (d.sub_results ?? []).filter((s) => s.url !== d.url).slice(0, 3);
        const title = d.meta.title ?? d.url;
        return `<li><a href="${clean(d.url)}"><strong>${title}</strong><span>${d.excerpt}</span></a>${
          subs.length
            ? `<ul>${subs.map((s) => `<li><a href="${clean(s.url)}"><strong>${s.title}</strong><span>${s.excerpt}</span></a></li>`).join('')}</ul>`
            : ''
        }</li>`;
      })
      .join('');
  }

  trigger.addEventListener('click', open);
  dialog.querySelector('form')!.addEventListener('submit', (e) => e.preventDefault());
  dialog.querySelector('[data-search-close]')!.addEventListener('click', () => dialog.close());
  input.addEventListener('input', run);
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      dialog.close();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive(active + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive(active - 1);
    } else if (e.key === 'Enter') {
      const a = list.querySelectorAll<HTMLAnchorElement>('a')[Math.max(active, 0)];
      if (a) {
        e.preventDefault();
        a.click();
      }
    }
  });
  dialog.addEventListener('click', (e) => {
    if (e.target === dialog) dialog.close();
    if ((e.target as HTMLElement).closest('a')) dialog.close();
  });
  document.addEventListener('keydown', (e) => {
    const t = e.target as HTMLElement;
    const typing = t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName);
    if ((e.key === '/' && !typing) || (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey))) {
      e.preventDefault();
      open();
    }
  });
}
