#!/usr/bin/env python3
"""Draw the PJDesk Chat window icon and rasterize every size from SVG.

A single rounded chat bubble in the in-app accent (#0A84FF), with the same
thin dark outline the other PJDesk marks use so it stays crisp on a light or
dark taskbar. Sizes 16 and 24 use a bolder, simpler drawing (no interior
lines) so the tray and taskbar stay legible.
"""
import io
import os
import struct

import cairosvg
import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
ACCENT = "#0A84FF"
ACCENT_HI = "#4DA8FF"
INK = "#080A10"
WHITE = "#FFFFFF"
SIZES = (16, 24, 32, 48, 64, 128, 256)
SHOTS = os.path.join(ROOT, "target-shots", "chat-icon")
ASSETS = os.path.join(ROOT, "crates", "pjdesk", "assets")
DOCS = os.path.join(ROOT, "docs")
LIGHT_BG = (244, 246, 248, 255)
DARK_BG = (0x1E, 0x1E, 0x1E, 255)
TASKBAR_LIGHT = (243, 243, 243, 255)
TASKBAR_DARK = (32, 32, 32, 255)


def wrap(body: str) -> str:
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" '
        'shape-rendering="geometricPrecision">\n'
        f"{body}\n</svg>\n"
    )


def bubble_path(bold: bool) -> str:
    # Slightly inset from the canvas so the outline and tail stay inside 256.
    # Tail sits at the lower left, like a sent/received chat bubble.
    if bold:
        # Chunky tail and a shorter body so 16/24px still read as a bubble.
        body = (
            "M 36 28 "
            "C 36 14 52 8 74 8 "
            "L 186 8 "
            "C 216 8 232 24 232 54 "
            "L 232 126 "
            "C 232 156 214 172 184 172 "
            "L 108 172 "
            "L 36 244 "
            "L 80 172 "
            "C 52 172 36 152 36 124 "
            "Z"
        )
    else:
        body = (
            "M 40 40 "
            "C 40 24 56 16 76 16 "
            "L 180 16 "
            "C 206 16 224 34 224 60 "
            "L 224 136 "
            "C 224 162 206 180 180 180 "
            "L 108 180 "
            "L 58 232 "
            "C 52 238 42 232 46 224 "
            "L 70 180 "
            "L 76 180 "
            "C 54 180 40 162 40 136 "
            "Z"
        )
    return body


def lines_body() -> str:
    # Three short "message" bars, left-aligned inside the bubble.
    bars = [
        (72, 70, 120, 16),
        (72, 98, 88, 16),
        (72, 126, 104, 16),
    ]
    parts = []
    for x, y, w, h in bars:
        r = h / 2
        parts.append(
            f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{WHITE}"/>'
        )
    return "".join(parts)


def highlight() -> str:
    return (
        '<path d="M 72 28 C 64 44 64 58 76 58 L 180 58 C 206 58 214 44 200 30 '
        'C 184 18 100 16 72 28 Z" fill="{hi}" fill-opacity="0.28"/>'.format(hi=ACCENT_HI)
    )


def svg_for(size: int) -> str:
    bold = size <= 24
    stroke = 14 if size <= 16 else 12 if size <= 24 else 10 if size <= 32 else 8
    d = bubble_path(bold)
    ink = (
        f'<path d="{d}" fill="{INK}" stroke="{INK}" stroke-width="{stroke:.2f}" '
        f'stroke-linejoin="round" stroke-linecap="round"/>'
    )
    fill = f'<path d="{d}" fill="{ACCENT}"/>'
    extras = ""
    if size >= 48:
        extras = highlight() + lines_body()
    elif size >= 32:
        extras = lines_body()
    return wrap(ink + fill + extras)



def pixel_bubble(size: int) -> np.ndarray:
    """A pixel-hinted bubble so 16 and 24 stay a chat mark, not a rounded square."""
    im = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    ink = (8, 10, 16, 255)
    blue = (10, 132, 255, 255)
    if size == 16:
        d.rounded_rectangle([0, 0, 15, 11], radius=4, fill=ink)
        d.rounded_rectangle([1, 1, 14, 10], radius=3, fill=blue)
        d.polygon([(2, 9), (8, 9), (2, 15)], fill=ink)
        d.polygon([(3, 8), (7, 8), (3, 13)], fill=blue)
    else:
        d.rounded_rectangle([0, 0, 23, 17], radius=6, fill=ink)
        d.rounded_rectangle([1, 1, 22, 16], radius=5, fill=blue)
        d.polygon([(3, 14), (12, 14), (3, 23)], fill=ink)
        d.polygon([(5, 13), (11, 13), (5, 20)], fill=blue)
    return np.asarray(im)


def raster(svg: str, size: int) -> np.ndarray:
    png = cairosvg.svg2png(
        bytestring=svg.encode("utf-8"),
        output_width=size,
        output_height=size,
        background_color="rgba(0,0,0,0)",
    )
    im = Image.open(io.BytesIO(png)).convert("RGBA")
    return np.asarray(im)


def save_png(arr, path):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    Image.fromarray(arr, "RGBA").save(path, optimize=True)


def save_rgba(arr, path):
    open(path, "wb").write(np.ascontiguousarray(arr).tobytes())


def _png_bytes(arr):
    buf = io.BytesIO()
    Image.fromarray(arr, "RGBA").save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def save_ico(path, images):
    blobs = [_png_bytes(im) for im in images]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for im, blob in zip(images, blobs):
        w = 0 if im.shape[1] >= 256 else im.shape[1]
        h = 0 if im.shape[0] >= 256 else im.shape[0]
        out += struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(blob), offset)
        offset += len(blob)
    out += b"".join(blobs)
    open(path, "wb").write(out)


def on_plate(arr, plate):
    rgb = arr[..., :3].astype(np.float32)
    a = arr[..., 3:4].astype(np.float32) / 255.0
    bg = np.array(plate[:3], np.float32)
    out = rgb * a + bg * (1 - a)
    return np.concatenate(
        [out.astype(np.uint8), np.full(arr.shape[:2] + (1,), 255, np.uint8)],
        axis=2,
    )


def verify(images):
    for size, arr in images.items():
        assert arr.shape == (size, size, 4), size
        # Corners stay transparent so the taskbar does not get a square halo.
        for x, y in ((0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)):
            assert arr[y, x, 3] == 0, f"corner {x},{y} at {size} should be transparent"
        opaque = int((arr[..., 3] > 200).sum())
        assert opaque > size * size * 0.18, f"{size}px icon is too empty ({opaque})"
        blue = int(
            (
                (arr[..., 3] > 200)
                & (arr[..., 2] > 160)
                & (arr[..., 2] > arr[..., 0].astype(np.int16) + 40)
            ).sum()
        )
        assert blue > max(8, size), f"{size}px is missing the accent blue ({blue})"


def write_previews(images):
    # A strip of every requested size on a light and a dark taskbar plate.
    show = (16, 24, 32, 48, 256)
    pad = 24
    gap = 20
    label_h = 28
    row_w = pad * 2 + 256
    # Place each size in a 256-wide cell? Better: pack at native size with gaps.
    content_w = pad * 2 + sum(show) + gap * (len(show) - 1)
    content_h = pad * 2 + 256 + label_h
    for name, plate in (("preview-light.png", TASKBAR_LIGHT), ("preview-dark.png", TASKBAR_DARK)):
        canvas = Image.new("RGBA", (content_w, content_h), plate)
        draw = ImageDraw.Draw(canvas)
        x = pad
        ink = (20, 20, 20, 255) if plate[0] > 128 else (230, 230, 230, 255)
        for size in show:
            y = pad + (256 - size)
            sprite = Image.fromarray(images[size], "RGBA")
            canvas.alpha_composite(sprite, (x, y))
            label = f"{size}px"
            tw = draw.textlength(label)
            draw.text((x + (size - tw) / 2, pad + 256 + 6), label, fill=ink)
            x += size + gap
        canvas.save(os.path.join(SHOTS, name), optimize=True)


def main():
    os.makedirs(SHOTS, exist_ok=True)
    os.makedirs(ASSETS, exist_ok=True)
    os.makedirs(DOCS, exist_ok=True)
    master = svg_for(256)
    open(os.path.join(DOCS, "pjdesk-chat-icon.svg"), "w").write(master)
    images = {size: (pixel_bubble(size) if size <= 24 else raster(svg_for(size), size)) for size in SIZES}
    verify(images)
    for size in (16, 24, 32, 48, 256):
        save_png(images[size], os.path.join(SHOTS, f"icon-{size}.png"))
    write_previews(images)
    save_rgba(images[256], os.path.join(ASSETS, "chat_icon_256.rgba"))
    save_rgba(images[32], os.path.join(ASSETS, "chat_icon_32.rgba"))
    save_png(images[256], os.path.join(ASSETS, "chat_icon_256.png"))
    save_ico(os.path.join(ASSETS, "chat.ico"), [images[s] for s in SIZES])
    print("wrote chat icon assets and", SHOTS)


if __name__ == "__main__":
    main()
