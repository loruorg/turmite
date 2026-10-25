# Turmite brand

The mark is a T drawn on a 3 x 3 grid of cells. A turmite is a Turing machine that walks a grid and turns cells black. The black cells form the T, and the last cell, with its round end, is the turmite.

## Files

All files are in `public/`. Run `pnpm brand` to make them again from `scripts/brand.mjs`.

| File | Use |
|---|---|
| `logo.svg` | The mark. Fill is `currentColor`. |
| `logo-wordmark.svg` | The mark and "turmite" in Inter 600, outlined. Fill is `currentColor`. |
| `favicon.svg` | The mark on a 16 px pixel grid. Black, white when the browser UI is dark. |
| `favicon.ico` | 16, 32 and 48 px, black. |
| `apple-touch-icon.png` | 180 x 180, white mark on black. |
| `og.png` | 1200 x 630 social image. |

The binary embeds a copy of `logo.svg` in `crates/turmite/assets/`.

## Geometry

- Grid: 3 x 3 cells. Cell 4 units, gap 1 unit. The mark is 14 x 14 units.
- Black cells: the top row and the middle cell. The bottom middle cell is the head: square top, half-circle bottom.
- Favicon: cells of 4 px with 1 px gaps and a 1 px margin. The head is rounded in whole pixels.
- Wordmark: the mark height is the cap height of the text. The gap between mark and text is 0.34 x the mark width. Text tracking is -0.025 em.

## Clear space and size

- Keep clear space of one cell (4 units, about 30 % of the mark width) on each side.
- Smallest size: 16 px for the mark, 80 px wide for the wordmark.

## Colour

- Black `#000` on white `#fff`, or white on black. No other colours, gradients, outlines or shadows.
- In SVG, use `currentColor` so the mark takes the text colour.
- The checkout uses green `#0b7a36` for paid and red `#c42b1c` for failed. These are status colours, not brand colours.

## Type

- Inter (variable), self-hosted. Headings 650 to 660 weight, tracking -0.03 em to -0.05 em.
- JetBrains Mono (variable), self-hosted, for code, labels and numbers.

## Do not

- Do not rotate, skew or recolour the mark.
- Do not change the cell gaps or round the square cells.
- Do not set the wordmark in another typeface.
