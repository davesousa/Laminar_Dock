# Laminar logo: Glass L

Chosen by Dave on 2026-11-16 from three rounds of concepts. The concepts were made with the
[logo-design skill](https://github.com/kaankiziltug/logo-design-skill) process: brief, category research,
concepts in black, audit and size tests, checkpoint, then this kit.

**The idea in one sentence:** two rounded bars overlap like panes of glass to make an L, and the overlap is left clear.

## Files

| File | Use it for |
| --- | --- |
| `kit/laminar-symbol.svg` | **Master.** One colour, and the overlap is a real hole. Recolour this one. |
| `kit/laminar-symbol-small.svg` | Small-size cut with thicker bars (72 instead of 64). Use it below about 32 px. |
| `kit/laminar-symbol-color.svg` | Full colour on light backgrounds: Laminar Blue, with the overlap at 40 % (the "glass"). |
| `kit/laminar-symbol-white-glass.svg` | On dark or photo backgrounds: white, with the overlap at 42 %. |
| `kit/laminar-app-icon.svg` | App icon and website logo: white glass L on a Laminar Blue squircle tile. |
| `kit/laminar-favicon.svg` | The app icon redrawn with the small-size cut, for browser tabs. |
| `kit/laminar-lockup.svg`, `kit/laminar-lockup-white.svg` | Mark and wordmark side by side. ⚠️ The wordmark is live text (Geist 600). Convert it to outlines before print or for any place where the font may be missing. |
| `kit/export/` | Generated variants: black, white, Laminar Blue, square, PNGs at 64 and 512, `favicon.ico`, touch and PWA icons. |
| `exploration/` | Rounds 1 to 3, kept for the record. |

The website (private repo `laminar-site`, a sibling folder of this one) uses copies in its `brand/` folder: `logo.svg` (the app icon), `favicon.ico`, `favicon.svg` and `apple-touch-icon.png`.

## Colour

| Name | HEX | RGB | Use |
| --- | --- | --- | --- |
| Laminar Blue | `#3366FF` | 51, 102, 255 | The mark on light backgrounds, and the app-icon tile (shaded `#4F82FF` → `#2A55F0`) |
| Ink | `#0B0C10` | 11, 12, 16 | Wordmark on light backgrounds |
| White | `#FFFFFF` | 255, 255, 255 | Mark and wordmark on dark backgrounds |

The glass overlap is always the mark's own colour at 40–42 % opacity. In one-colour use (black, white, embroidery, print), the overlap is a hole.

## Rules

- **Clear space:** keep at least half a bar's width (32 units on the 256 grid) empty around the mark.
- **Minimum size:** 16 px using the small cut; 32 px and up using the master.
- **Don't:** stretch it, rotate it, outline it, add shadows or glows, fill the overlap solid, or put the colour version on a busy photo (use the white version).

## Checks run

- `svg_audit.py`: the master and the small cut both score 100/100. The app icon gets two expected warnings: its gradient tile, and the anchor count of its smooth squircle outline.
- Rendered and inspected at 256, 64, 32 and 16 px, including the favicon.
- Readings test: no unintended shapes found.

## Not done yet

- Trademark clearance: run a professional search before relying on the mark.
- The wordmark hasn't been converted to outlines.
- No presentation board yet (the skill's `presentation_board.py`).
- Still to update: the app's own icon (`native/`), the README banner and `docs/media` graphics, and the installer icon.
