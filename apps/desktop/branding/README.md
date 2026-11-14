# Branding

The mark: a heavy D whose bowl becomes two crossing arrows, black and white only.

- `render.swift`: the geometry, and every icon drawn from it (Core Graphics).
- `make-icons.py`: runs it and puts each file where it is used: `src-tauri/icons/` (the Mac
  `icon.icns` and PNGs on Apple's grid, Windows `icon.ico` and `Square*Logo.png`, the menu bar's
  `tray.png`), `site/public/icon.png` and `favicon.png`, and `icon-1024.png` here.
  `python3 apps/desktop/branding/make-icons.py` (macOS: needs swift and iconutil).
- `svg_mark.py`: the same geometry as SVG paths, used to write the SVGs below. Change it together
  with `render.swift`.
- `mark.svg`: the bare mark (docs).
- `icon-dark.svg`: the icon as a picture, white mark on a black tile to the edges (README).
- `logo-light-framed.svg`: the mark inside a gapped rounded frame, for light backgrounds.

Geometry (1024 box, y down): stroke 100, round caps and joins; the two arrow paths are mirror
images about y = 512, so their tails match; the arrowheads are triangles whose tips sit on the
D's top and bottom lines. (A redrawing with rounded heads pointing nearly right went out in
0.1.17 and was taken back in 0.1.18.)

The Mac icon follows Apple's grid: an 824 tile centred in the 1024 canvas, transparent around it,
with a soft shadow. macOS 11 to 15 show an icon as drawn, so a tile to the edges looks a size
too big there and shows whatever its corners hold; macOS 26 and later mask every icon themselves.
