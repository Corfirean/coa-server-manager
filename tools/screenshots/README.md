# Localized screenshot sources

`localized-captions.json` contains reviewed-language copy for the ten annotated cards. Keys use runtime locale `zh`; published assets use `zh-CN`.

Capture the actual interface in each locale using an isolated demo, never production accounts or databases. Keep example character names readable. Save browser captures as JPEG in `raw/<locale>/<scene>.jpg` and their visible DOM geometry in matching JSON files (`layout.cards`, `buttons`, `table`, `filters`, `characters`). The renderer reads these rectangles to keep numbered highlights aligned with translated layouts.

Run `python tools/screenshots/build-gallery.py --capture-root <capture-folder>` (Pillow required). Open the generated HTML in a browser at a fixed 1500-pixel width and export each whole card. Verify dimensions, full content and text wrapping before copying images to `docs/screenshots/<locale>/`. Keep the three account/character guide captures and two Wildcard captures in each language too. Update matching README and guide links together. Preserve experimental Portable Characters notices.

These files render existing screenshots and HTML annotations; they do not synthesize interface screenshots. English assets remain unchanged unless explicitly refreshed.
