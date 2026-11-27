"""Draws the Moviclone app icon and writes the files Tauri bundles.

    python desktop/icons/make_icons.py

No dependencies: the mark (a blue rounded square with a white "M" line, the
same as the HMI and studio favicon) is rasterised with supersampling, and the
.ico / .icns containers hold PNG images, which Windows and macOS both accept.
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
BLUE = (31, 111, 224)
BLUE_DARK = (18, 78, 170)
WHITE = (255, 255, 255)


def dist_to_segment(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)))
    return math.hypot(px - ax - t * vx, py - ay - t * vy)


def coverage_rounded_rect(x, y, size, inset, radius):
    """1 inside the rounded square, 0 outside (point test)."""
    lo, hi = inset, size - inset
    if x < lo or x > hi or y < lo or y > hi:
        return 0.0
    cx = min(max(x, lo + radius), hi - radius)
    cy = min(max(y, lo + radius), hi - radius)
    return 1.0 if math.hypot(x - cx, y - cy) <= radius else 0.0


def render(size, ss=4):
    """RGBA bytes of the icon at `size` px."""
    # The favicon's geometry in a 32-unit box: M6 22 V10 l5 7 5-7 5 7 5-7 v12, stroke 3.
    pts = [(6, 22), (6, 10), (11, 17), (16, 10), (21, 17), (26, 10), (26, 22)]
    margin = size * 0.06
    inner = size - 2 * margin
    scale = inner / 32.0
    path = [(margin + x * scale, margin + y * scale) for x, y in pts]
    half = 1.5 * scale * 1.05
    radius = inner * 0.22
    rows = []
    for j in range(size):
        row = bytearray()
        for i in range(size):
            bg = fg = 0.0
            for sj in range(ss):
                for si in range(ss):
                    x = i + (si + 0.5) / ss
                    y = j + (sj + 0.5) / ss
                    c = coverage_rounded_rect(x, y, size, margin, radius)
                    if not c:
                        continue
                    bg += 1
                    d = min(dist_to_segment(x, y, *path[k], *path[k + 1]) for k in range(len(path) - 1))
                    if d <= half:
                        fg += 1
            n = ss * ss
            a = bg / n
            if a == 0:
                row += bytes((0, 0, 0, 0))
                continue
            f = fg / max(bg, 1)
            # A slight vertical shade on the blue.
            t = j / size
            base = tuple(round(BLUE[k] * (1 - 0.35 * t) + BLUE_DARK[k] * 0.35 * t) for k in range(3))
            rgb = tuple(round(base[k] * (1 - f) + WHITE[k] * f) for k in range(3))
            row += bytes((*rgb, round(a * 255)))
        rows.append(bytes(row))
    return rows


def png(size):
    rows = render(size, ss=4 if size <= 256 else 2)
    raw = b''.join(b'\x00' + r for r in rows)

    def chunk(tag, data):
        return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


def ico(images):
    """ICO with PNG entries: images = {size: png_bytes}."""
    sizes = sorted(images)
    header = struct.pack('<HHH', 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries, data = b'', b''
    for s in sizes:
        b = images[s]
        entries += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(b), offset + len(data))
        data += b
    return header + entries + data


def icns(images):
    """ICNS with PNG entries (ic07 128, ic08 256, ic09 512, ic10 1024)."""
    kinds = {128: b'ic07', 256: b'ic08', 512: b'ic09', 1024: b'ic10'}
    body = b''.join(kinds[s] + struct.pack('>I', 8 + len(images[s])) + images[s] for s in sorted(kinds) if s in images)
    return b'icns' + struct.pack('>I', 8 + len(body)) + body


def main():
    images = {s: png(s) for s in (16, 24, 32, 48, 64, 128, 256, 512, 1024)}
    write = lambda name, data: open(os.path.join(HERE, name), 'wb').write(data)
    write('32x32.png', images[32])
    write('128x128.png', images[128])
    write('128x128@2x.png', images[256])
    write('icon.png', images[512])
    write('icon.ico', ico({s: images[s] for s in (16, 24, 32, 48, 64, 128, 256)}))
    write('icon.icns', icns(images))
    print('icons written to', HERE)


if __name__ == '__main__':
    main()
