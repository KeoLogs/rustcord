#!/usr/bin/env python3
"""Rebuild the RustCord app icons from rustcord.svg.

Needs `rsvg-convert` (librsvg) on PATH and Pillow (`pip install pillow`).
Run from anywhere:  python3 assets/icon/generate.py

Writes, next to this script:
  png/rustcord-<size>.png   16 ... 1024 px (256 is the window icon)
  rustcord.ico              Windows .exe icon (16-256 px)
  rustcord.icns             macOS .app icon
"""

import subprocess
import sys
from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SVG = HERE / "rustcord.svg"
PNG_DIR = HERE / "png"
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def render(size: int) -> Path:
    out = PNG_DIR / f"rustcord-{size}.png"
    subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), str(SVG), "-o", str(out)],
        check=True,
    )
    # Re-save through Pillow so the files are plain, stripped RGBA PNGs.
    Image.open(out).convert("RGBA").save(out, optimize=True)
    return out


def main() -> int:
    PNG_DIR.mkdir(exist_ok=True)
    pngs = {size: render(size) for size in SIZES}

    # Windows: one .ico with every size; Pillow picks each frame from the
    # nearest larger source, so hand it the rendered images directly.
    images = [Image.open(pngs[s]) for s in ICO_SIZES]
    images[-1].save(
        HERE / "rustcord.ico",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=images[:-1],
    )

    # macOS: Pillow builds every icns size from the 1024 px render.
    Image.open(pngs[1024]).save(HERE / "rustcord.icns")

    print(f"wrote {len(pngs)} PNGs, rustcord.ico and rustcord.icns in {HERE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
