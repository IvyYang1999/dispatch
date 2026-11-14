"""The mark's SVG paths, from the same geometry as render.swift (1024 box, y down): the original
mark of 2026-11-04. Prints the <g> that draws the mark in one colour; `python3 svg_mark.py '#ffffff'`."""
import sys

W = 100
shift_x = -50
STEMS = [
    'M 270 734 V 290 H 380 C 530 290, 575 420, 610 512 C 625 549, 728 634, 780 664',
    'M 270 734 H 380 C 530 734, 575 604, 610 512 C 625 475, 728 390, 780 360',
]
HEADS = ['M 840 560 L 901 734 L 720 768 Z', 'M 840 464 L 901 290 L 720 256 Z']


def group(color, indent='  '):
    lines = [f'{indent}<g transform="translate({shift_x} 0)">',
             f'{indent}  <g fill="none" stroke="{color}" stroke-width="{W}" stroke-linecap="round" stroke-linejoin="round">']
    lines += [f'{indent}    <path d="{d}"/>' for d in STEMS]
    lines += [f'{indent}  </g>', f'{indent}  <g fill="{color}">']
    lines += [f'{indent}    <path d="{d}"/>' for d in HEADS]
    lines += [f'{indent}  </g>', f'{indent}</g>']
    return '\n'.join(lines)


if __name__ == '__main__':
    print(group(sys.argv[1] if len(sys.argv) > 1 else '#111111'))
