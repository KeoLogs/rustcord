#!/usr/bin/env python3
"""Writes bubbles.png: soft glassy bubbles on a transparent background.

Procedural (no source images), standard library only. Regenerate with:
    python3 make_bubbles.py
"""
import math
import struct
import zlib

W, H = 640, 640
SEED = 7


def rng(seed):
    state = seed
    while True:
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        yield state / 0x7FFFFFFF


def smooth(e0, e1, x):
    t = max(0.0, min(1.0, (x - e0) / (e1 - e0)))
    return t * t * (3 - 2 * t)


def main():
    r = rng(SEED)
    bubbles = []
    for _ in range(28):
        rad = 12 + next(r) ** 2 * 80
        bubbles.append((next(r) * W, next(r) * H, rad))
    # RGBA floats, premultiplied while compositing.
    px = [[0.0, 0.0, 0.0, 0.0] for _ in range(W * H)]
    for cx, cy, rad in bubbles:
        x0, x1 = max(0, int(cx - rad - 2)), min(W, int(cx + rad + 2))
        y0, y1 = max(0, int(cy - rad - 2)), min(H, int(cy + rad + 2))
        hx, hy, hr = cx - rad * 0.38, cy - rad * 0.42, rad * 0.32
        for y in range(y0, y1):
            for x in range(x0, x1):
                d = math.hypot(x + 0.5 - cx, y + 0.5 - cy) / rad
                if d > 1.0:
                    continue
                edge = 1.0 - smooth(0.97, 1.0, d)  # anti-aliased outline
                rim = smooth(0.70, 0.97, d) * 0.75
                body = 0.10
                hd = math.hypot(x + 0.5 - hx, y + 0.5 - hy) / hr
                spot = (1.0 - smooth(0.0, 1.0, hd)) * 0.85
                a = min(1.0, (body + rim + spot)) * edge
                # Rim slightly aqua, highlight white.
                col = (0.86 + 0.14 * spot, 0.97 + 0.03 * spot, 1.0)
                p = px[y * W + x]
                inv = 1.0 - a
                p[0] = col[0] * a + p[0] * inv
                p[1] = col[1] * a + p[1] * inv
                p[2] = col[2] * a + p[2] * inv
                p[3] = a + p[3] * inv
    raw = bytearray()
    for y in range(H):
        raw.append(0)
        for x in range(W):
            pr, pg, pb, pa = px[y * W + x]
            if pa <= 0.0:
                raw += b"\0\0\0\0"
                continue
            q = lambda c: max(0, min(255, round(c / pa * 255)))
            raw += bytes((q(pr), q(pg), q(pb), round(pa * 255)))

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open("bubbles.png", "wb") as f:
        f.write(png)
    print(f"bubbles.png: {len(png)} bytes")


if __name__ == "__main__":
    main()
