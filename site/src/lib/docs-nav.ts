// Sidebar order. Each slug is a file in src/content/docs/<slug>.md ("index" is /docs/).
export const docsNav: { section: string; items: string[] }[] = [
  { section: 'Start', items: ['index', 'quick-start', 'install', 'first-payment', 'migrate'] },
  {
    section: 'Integrate',
    items: [
      'merchant-api',
      'api-reference',
      'webhooks',
      'checkout',
      'payouts',
      'exchange',
      'networks',
      'errors',
      'sdk',
      'woocommerce',
    ],
  },
  {
    section: 'Operate',
    items: [
      'configuration',
      'dashboard',
      'projects',
      'nodes',
      'wallets',
      'path-prefix',
      'reverse-proxy',
      'private-dashboard',
      'backups',
      'upgrades',
      'monitoring',
      'troubleshooting',
      'go-live',
    ],
  },
  { section: 'Security', items: ['security-model', 'architecture', 'threat-model', 'report-vulnerability'] },
  { section: 'Reference', items: ['cli', 'environment', 'statuses', 'changelog'] },
  { section: 'Community', items: ['contributing', 'license'] },
];

export const docsOrder: string[] = docsNav.flatMap((s) => s.items);

export function docsHref(slug: string): string {
  return slug === 'index' ? '/docs/' : `/docs/${slug}/`;
}

export function docsMdHref(slug: string): string {
  return slug === 'index' ? '/docs/index.md' : `/docs/${slug}.md`;
}

export function sectionOf(slug: string): string | undefined {
  return docsNav.find((s) => s.items.includes(slug))?.section;
}
