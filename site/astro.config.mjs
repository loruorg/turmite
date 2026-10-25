import { defineConfig, fontProviders } from 'astro/config';
import sitemap from '@astrojs/sitemap';
import { satteri } from '@astrojs/markdown-satteri';
import { monoLight, monoDark } from './src/lib/shiki-themes.mjs';
import { stripVerify } from './src/lib/markdown-plugins.mjs';

export default defineConfig({
  site: 'https://turmite.com',
  trailingSlash: 'ignore',
  build: { format: 'directory' },
  // Scripts stay external (strict CSP). The three.js chunk loads on demand, so its size does not block the page.
  vite: { build: { assetsInlineLimit: 0, chunkSizeWarningLimit: 600 } },
  integrations: [sitemap({ filter: (page) => !page.endsWith('/404/') })],
  markdown: {
    processor: satteri({ features: { smartPunctuation: false }, hastPlugins: [stripVerify] }),
    shikiConfig: {
      themes: { light: monoLight, dark: monoDark },
      langAlias: { caddyfile: 'text', env: 'dotenv' },
    },
  },
  fonts: [
    {
      provider: fontProviders.local(),
      name: 'Inter',
      cssVariable: '--font-sans',
      fallbacks: ['system-ui', 'sans-serif'],
      options: {
        variants: [
          {
            src: ['@fontsource-variable/inter/files/inter-latin-wght-normal.woff2'],
            weight: '100 900',
            style: 'normal',
          },
        ],
      },
    },
    {
      provider: fontProviders.local(),
      name: 'JetBrains Mono',
      cssVariable: '--font-mono',
      fallbacks: ['ui-monospace', 'monospace'],
      options: {
        variants: [
          {
            src: ['@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2'],
            weight: '100 800',
            style: 'normal',
          },
        ],
      },
    },
  ],
});
