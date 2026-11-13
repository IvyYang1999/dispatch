"""The mark's SVG paths, from the same geometry as render.swift (1024 box, y down).
Prints the <g> that draws the mark in one colour; `python3 svg_mark.py '#ffffff'`."""
import math, sys

W = 100
angle = math.radians(8)
stem_angle = math.radians(30)
tail = (748.0, 700.0)
head_len, head_half, head_back, head_round, tip_round = 160.0, 100.0, 26.0, 20.0, 14.0
shift_x = -44


def f(v):
    s = f'{v:.1f}'
    return s[:-2] if s.endswith('.0') else s


def pt(p):
    return f'{f(p[0])} {f(p[1])}'


def flip(p):
    return (p[0], 1024 - p[1])


def stem(m):
    d = (math.cos(stem_angle), math.sin(stem_angle))
    c2 = (tail[0] - 80 * d[0], tail[1] - 80 * d[1])
    P = lambda x, y: m((x, y))
    return (f'M {pt(P(270, 734))} V {f(P(0, 290)[1])} H 380 '
            f'C {pt(P(530, 290))}, {pt(P(575, 420))}, {pt(P(610, 512))} '
            f'C {pt(P(638, 585))}, {pt(m(c2))}, {pt(m(tail))}')


def head(m):
    d = (math.cos(angle), math.sin(angle))
    n = (-d[1], d[0])
    base = (tail[0] - head_back * d[0], tail[1] - head_back * d[1])
    tip = (tail[0] + head_len * d[0], tail[1] + head_len * d[1])
    a = (base[0] + head_half * n[0], base[1] + head_half * n[1])
    b = (base[0] - head_half * n[0], base[1] - head_half * n[1])
    pts = [m(a), m(tip), m(b)]
    radii = [head_round, tip_round, head_round]
    out = []
    for i, P in enumerate(pts):
        prev, nxt = pts[i - 1], pts[(i + 1) % 3]
        v1 = (prev[0] - P[0], prev[1] - P[1])
        v2 = (nxt[0] - P[0], nxt[1] - P[1])
        l1, l2 = math.hypot(*v1), math.hypot(*v2)
        cosang = (v1[0] * v2[0] + v1[1] * v2[1]) / (l1 * l2)
        theta = math.acos(max(-1, min(1, cosang)))
        t = radii[i] / math.tan(theta / 2)
        t1 = (P[0] + v1[0] / l1 * t, P[1] + v1[1] / l1 * t)
        t2 = (P[0] + v2[0] / l2 * t, P[1] + v2[1] / l2 * t)
        vin = (P[0] - prev[0], P[1] - prev[1])
        vout = (nxt[0] - P[0], nxt[1] - P[1])
        sweep = 1 if vin[0] * vout[1] - vin[1] * vout[0] > 0 else 0
        out.append((t1, t2, radii[i], sweep))
    s = f'M {pt(out[0][1])}'
    for t1, t2, r, sweep in out[1:] + out[:1]:
        s += f' L {pt(t1)} A {f(r)} {f(r)} 0 0 {sweep} {pt(t2)}'
    return s + ' Z'


def group(color, indent='  '):
    same = lambda p: p
    return '\n'.join([
        f'{indent}<g transform="translate({shift_x} 0)">',
        f'{indent}  <g fill="none" stroke="{color}" stroke-width="{W}" stroke-linecap="round" stroke-linejoin="round">',
        f'{indent}    <path d="{stem(same)}"/>',
        f'{indent}    <path d="{stem(flip)}"/>',
        f'{indent}  </g>',
        f'{indent}  <g fill="{color}">',
        f'{indent}    <path d="{head(same)}"/>',
        f'{indent}    <path d="{head(flip)}"/>',
        f'{indent}  </g>',
        f'{indent}</g>',
    ])


if __name__ == '__main__':
    print(group(sys.argv[1] if len(sys.argv) > 1 else '#111111'))
