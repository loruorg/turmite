// Monochrome code themes. Weight and tone carry the syntax, not hue.
function theme(name, type, c) {
  return {
    name,
    type,
    colors: { 'editor.background': c.bg, 'editor.foreground': c.fg },
    fg: c.fg,
    bg: c.bg,
    tokenColors: [
      { settings: { foreground: c.fg, background: c.bg } },
      { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: c.muted, fontStyle: 'italic' } },
      {
        scope: ['keyword', 'storage', 'storage.type', 'keyword.control', 'keyword.operator.new', 'variable.language'],
        settings: { foreground: c.strong, fontStyle: 'bold' },
      },
      { scope: ['string', 'string.quoted', 'string.template', 'markup.inline.raw'], settings: { foreground: c.string } },
      { scope: ['constant.numeric', 'constant.language', 'constant.character'], settings: { foreground: c.string } },
      { scope: ['entity.name.function', 'support.function', 'entity.name.type', 'entity.name.class'], settings: { foreground: c.strong } },
      { scope: ['support.type.property-name', 'meta.object-literal.key', 'entity.name.tag', 'variable.other.property'], settings: { foreground: c.strong } },
      { scope: ['punctuation', 'meta.brace', 'keyword.operator'], settings: { foreground: c.punct } },
      { scope: ['variable.parameter', 'variable.other.env'], settings: { foreground: c.fg, fontStyle: 'italic' } },
      { scope: ['markup.inserted'], settings: { foreground: c.strong, fontStyle: 'bold' } },
      { scope: ['markup.deleted'], settings: { foreground: c.muted, fontStyle: 'strikethrough' } },
      { scope: ['entity.other.attribute-name'], settings: { foreground: c.string } },
    ],
  };
}

export const monoLight = theme('turmite-light', 'light', {
  bg: '#f6f6f6',
  fg: '#1a1a1a',
  strong: '#000000',
  string: '#4d4d4d',
  muted: '#767676',
  punct: '#6b6b6b',
});

export const monoDark = theme('turmite-dark', 'dark', {
  bg: '#141414',
  fg: '#e6e6e6',
  strong: '#ffffff',
  string: '#b8b8b8',
  muted: '#8f8f8f',
  punct: '#9a9a9a',
});
