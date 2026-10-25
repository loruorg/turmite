// Docs page behaviour. Every feature here is optional: the page reads and works without it.

const COPY = '<svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.8"/><path d="M16 8V4H4v12h4" fill="none" stroke="currentColor" stroke-width="1.8"/></svg>';
const CHECK = '<svg width="16" height="16" viewBox="0 0 24 24" aria-hidden="true"><path d="m4.5 12.5 5 5 10-11" fill="none" stroke="currentColor" stroke-width="2"/></svg>';

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

function flash(btn: HTMLElement, label: string, ms = 1600) {
  const html = btn.innerHTML;
  btn.innerHTML = CHECK + (label ? `<span>${label}</span>` : '');
  btn.dataset.done = '';
  window.setTimeout(() => {
    btn.innerHTML = html;
    delete btn.dataset.done;
  }, ms);
}

// Code blocks: language label and copy button.
for (const pre of document.querySelectorAll<HTMLPreElement>('.prose pre')) {
  if (pre.parentElement?.classList.contains('code')) continue;
  const wrap = document.createElement('div');
  wrap.className = 'code';
  pre.replaceWith(wrap);
  wrap.append(pre);
  const lang = pre.dataset.language;
  if (lang && lang !== 'plaintext' && lang !== 'text') {
    const tag = document.createElement('span');
    tag.className = 'code-lang';
    tag.textContent = lang;
    wrap.append(tag);
  }
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'code-copy';
  btn.setAttribute('aria-label', 'Copy code');
  btn.innerHTML = COPY;
  btn.addEventListener('click', async () => {
    if (await copyText(pre.innerText.replace(/\n$/, ''))) flash(btn, '');
  });
  wrap.append(btn);
}

// Heading anchors.
for (const h of document.querySelectorAll<HTMLHeadingElement>('.prose h2[id], .prose h3[id]')) {
  const a = document.createElement('a');
  a.className = 'anchor';
  a.href = `#${h.id}`;
  a.setAttribute('aria-label', `Link to ${h.textContent ?? 'section'}`);
  a.textContent = '#';
  h.append(a);
}

// Copy the page as Markdown.
const copyPage = document.querySelector<HTMLButtonElement>('[data-copy-page]');
if (copyPage) {
  copyPage.hidden = false;
  copyPage.addEventListener('click', async () => {
    const url = copyPage.dataset.copyPage!;
    try {
      const res = await fetch(url);
      if (res.ok && (await copyText(await res.text()))) flash(copyPage, 'Copied');
    } catch {
      window.location.href = url;
    }
  });
}

// Table of contents: mark the section in view.
const tocLinks = [...document.querySelectorAll<HTMLAnchorElement>('.toc a')];
if (tocLinks.length) {
  const byId = new Map(tocLinks.map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
  const heads = [...byId.keys()].map((id) => document.getElementById(id)).filter((el): el is HTMLElement => !!el);
  const update = () => {
    const top = 120;
    let current = heads[0];
    for (const h of heads) {
      if (h.getBoundingClientRect().top - top <= 0) current = h;
      else break;
    }
    for (const a of tocLinks) a.classList.toggle('active', byId.get(current?.id ?? '') === a);
  };
  let raf = 0;
  addEventListener(
    'scroll',
    () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(update);
    },
    { passive: true },
  );
  update();
}

// Theme toggle.
const toggle = document.querySelector<HTMLButtonElement>('[data-theme-toggle]');
if (toggle) {
  toggle.hidden = false;
  toggle.addEventListener('click', () => {
    const next = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem('theme', next);
    } catch {}
  });
}

// Close the mobile menu after a choice.
document.querySelector('.docs-menu nav')?.addEventListener('click', (e) => {
  if ((e.target as HTMLElement).closest('a')) (document.querySelector('.docs-menu') as HTMLDetailsElement).open = false;
});
