// Notes for maintainers (<!-- VERIFY: ... -->) stay in the source only.
export const stripVerify = {
  name: 'strip-verify',
  raw(node, ctx) {
    if (/^\s*<!--\s*VERIFY/.test(node.value)) ctx.removeNode(node);
  },
  comment(node, ctx) {
    if (/^\s*VERIFY/.test(node.value)) ctx.removeNode(node);
  },
};

export function stripVerifyText(md) {
  return md.replace(/^[ \t]*<!--\s*VERIFY[\s\S]*?-->[ \t]*\n?/gm, '').replace(/\n{3,}/g, '\n\n');
}
