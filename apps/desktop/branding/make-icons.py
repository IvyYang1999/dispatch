"""Draw every Dispatch icon from render.swift and put it where it is used (macOS only: it needs
swift and iconutil). Run from anywhere: python3 apps/desktop/branding/make-icons.py

- the Mac app icon (icon.icns): Apple's grid, an 824 tile centred in 1024 with a transparent
  margin and a soft shadow, so macOS 11 to 15 show it at the size of every other icon;
- the PNGs the bundle and Linux use (32, 64, 128, 128@2x, icon.png): the same Mac-grid drawing;
- Windows (icon.ico, Square*Logo, StoreLogo) and the web page (icon.png, favicon.png): the tile
  to the edges with transparent corners;
- the menu bar's template image (tray.png), and branding/icon-1024.png (the Mac icon at 1024,
  kept as the picture of record).

The SVGs (mark, icon-dark, logo-light-framed, docs/social-preview) take the same geometry from
svg_mark.py. docs/social-preview.png is not redrawn here: its type is set in SF, which the SVG
renderers on hand do not use.
"""
import os, shutil, struct, subprocess, sys, tempfile

here = os.path.dirname(os.path.abspath(__file__))
repo = os.path.abspath(os.path.join(here, '..', '..', '..'))
icons = os.path.join(repo, 'apps', 'desktop', 'src-tauri', 'icons')
site = os.path.join(repo, 'site', 'public')


def run(*cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)


def ico(pngs, out):
    """An .ico holding PNG images (Windows Vista and later read these)."""
    datas = [(n, open(p, 'rb').read()) for n, p in pngs]
    head = struct.pack('<HHH', 0, 1, len(datas))
    offset = 6 + 16 * len(datas)
    entries, body = b'', b''
    for n, data in datas:
        side = 0 if n >= 256 else n
        entries += struct.pack('<BBBBHHII', side, side, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    open(out, 'wb').write(head + entries + body)


with tempfile.TemporaryDirectory() as tmp:
    run('swift', os.path.join(here, 'render.swift'), tmp)
    mac = lambda n: os.path.join(tmp, f'icon-mac-{n}.png')
    full = lambda n: os.path.join(tmp, f'icon-full-{n}.png')

    iconset = os.path.join(tmp, 'icon.iconset')
    os.mkdir(iconset)
    for n in (16, 32, 128, 256, 512):
        shutil.copy(mac(n), os.path.join(iconset, f'icon_{n}x{n}.png'))
        shutil.copy(mac(n * 2), os.path.join(iconset, f'icon_{n}x{n}@2x.png'))
    run('iconutil', '-c', 'icns', iconset, '-o', os.path.join(icons, 'icon.icns'))

    for name, n in (('32x32.png', 32), ('64x64.png', 64), ('128x128.png', 128), ('128x128@2x.png', 256), ('icon.png', 512)):
        shutil.copy(mac(n), os.path.join(icons, name))
    for n in (30, 44, 71, 89, 107, 142, 150, 284, 310):
        shutil.copy(full(n), os.path.join(icons, f'Square{n}x{n}Logo.png'))
    shutil.copy(full(50), os.path.join(icons, 'StoreLogo.png'))
    ico([(n, full(n)) for n in (16, 24, 32, 48, 64, 256)], os.path.join(icons, 'icon.ico'))
    shutil.copy(os.path.join(tmp, 'tray.png'), os.path.join(icons, 'tray.png'))

    shutil.copy(full(256), os.path.join(site, 'icon.png'))
    shutil.copy(full(64), os.path.join(site, 'favicon.png'))
    shutil.copy(mac(1024), os.path.join(here, 'icon-1024.png'))


print('icons written', file=sys.stderr)
