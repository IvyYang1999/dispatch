# Branding

The mark: a heavy D whose bowl becomes two crossing arrows, black and white only.

- `mark.svg`: the bare mark (menu bar, notifications, docs).
- `icon-dark.svg`: the app icon, white mark on a black rounded tile.
- `logo-light-framed.svg`: the mark inside a gapped rounded frame, for light backgrounds.
- `icon-1024.png`: the rendered icon source; `npx tauri icon branding/icon-1024.png` regenerates `src-tauri/icons/`.

Geometry (1024 canvas): stroke 100, round caps and joins; the two arrow paths are mirror images about y = 512, so their tails match; the arrowheads are isosceles triangles whose tips sit on the D's top and bottom lines.
