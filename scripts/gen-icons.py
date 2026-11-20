#!/usr/bin/env python3
"""Regenerates assets/icon.png and assets/icon.ico from assets/icon.svg.

The SVG is the design; the PNG and the ICO are what Windows is handed. Chrome
renders it because the icon is text in Dubai / M PLUS 1p, and every ICO size is
downsampled from that one master so each slot gets its own bitmap.

    python scripts/gen-icons.py

Needs Pillow, and Chrome or Edge for the render.
"""

import os
import struct
import subprocess
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
MASTER = 2048
SIZES = [256, 128, 96, 64, 48, 40, 32, 24, 20, 16]
PNG_MIN = 256  # only 256 is PNG: .NET's Icon.ToBitmap and older shell code read BMP entries


def find_browser() -> str:
    candidates = [
        os.environ.get("CHROME_PATH", ""),
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ]
    for path in candidates:
        if path and Path(path).exists():
            return path
    sys.exit("no Chrome or Edge found; set CHROME_PATH")


def render(svg_path: Path, out_png: Path) -> Image.Image:
    svg = svg_path.read_text(encoding="utf-8")
    # The width/height attributes would win over the page CSS; keep only the viewBox.
    svg = svg.replace(
        ' viewBox="0 0 512 512" width="512" height="512"',
        ' viewBox="0 0 512 512" preserveAspectRatio="xMidYMid meet"',
        1,
    )
    page = ROOT / "target" / "icon-render.html"
    page.parent.mkdir(parents=True, exist_ok=True)
    page.write_text(
        "<!doctype html><style>html,body{margin:0;padding:0;background:transparent}"
        f"svg{{display:block;width:{MASTER}px;height:{MASTER}px}}</style>{svg}",
        encoding="utf-8",
    )
    subprocess.run(
        [
            find_browser(),
            "--headless",
            "--disable-gpu",
            "--force-device-scale-factor=1",
            "--default-background-color=00000000",
            "--hide-scrollbars",
            f"--window-size={MASTER},{MASTER}",
            f"--screenshot={out_png}",
            f"file:///{page.as_posix()}",
        ],
        check=True,
        capture_output=True,
        timeout=180,
    )
    img = Image.open(out_png).convert("RGBA")
    if img.size != (MASTER, MASTER):
        sys.exit(f"render came out {img.size}, expected {MASTER}")
    return img


def bmp_entry(img: Image.Image) -> bytes:
    """ICO-style DIB: BITMAPINFOHEADER, doubled height, BGRA bottom-up, 1bpp AND mask."""
    w, h = img.size
    px = img.load()
    xor = bytearray()
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[x, y]
            xor += bytes((b, g, r, a))
    mask_row = ((w + 31) // 32) * 4
    and_mask = bytearray()
    for y in range(h - 1, -1, -1):
        row = bytearray(mask_row)
        for x in range(w):
            if px[x, y][3] == 0:
                row[x // 8] |= 0x80 >> (x % 8)
        and_mask += row
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(xor) + len(and_mask), 0, 0, 0, 0)
    return header + bytes(xor) + bytes(and_mask)


def png_entry(img: Image.Image, scratch: Path) -> bytes:
    img.save(scratch, format="PNG", optimize=True)
    return scratch.read_bytes()


def write_ico(entries, out_path: Path) -> None:
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    body = bytearray()
    offset = 6 + 16 * len(entries)
    for size, data in entries:
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        body += data
        offset += len(data)
    out_path.write_bytes(bytes(out) + bytes(body))


def main() -> None:
    scratch = ROOT / "target" / "icon-scratch"
    scratch.mkdir(parents=True, exist_ok=True)
    master = render(ROOT / "assets" / "icon.svg", ROOT / "target" / "icon-render.png")

    # Self-check: the render must still be the rounded, transparent-cornered icon.
    assert master.getpixel((0, 0))[3] == 0, "render lost its transparent corner"
    assert master.split()[3].getextrema() == (0, 255), "render lost transparency"

    master.resize((512, 512), Image.LANCZOS).save(ROOT / "assets" / "icon.png", optimize=True)

    entries = []
    for size in SIZES:
        resized = master.resize((size, size), Image.LANCZOS)
        data = png_entry(resized, scratch / "entry.png") if size >= PNG_MIN else bmp_entry(resized)
        entries.append((size, data))
    write_ico(entries, ROOT / "assets" / "icon.ico")
    print(f"assets/icon.png 512x512, assets/icon.ico {len(SIZES)} sizes")


if __name__ == "__main__":
    main()
