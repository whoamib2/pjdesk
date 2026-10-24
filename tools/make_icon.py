#!/usr/bin/env python3
"""Build the PJDesk icon from the supplied artwork.

The source is a 1024x1024 PNG whose checkerboard is painted into the pixels
(it is not an alpha channel). This script drops that pattern, fills the holes
it leaves in the white monitor and server, crops the artwork to a tight
square, and writes the Windows, tray, Android, site, and README files.

The 16 and 24 pixel tray marks keep the monitor and cursor and drop the cable
and server box, which turn into noise at that size.
"""
import io
import os
import struct
import sys

import numpy as np
from PIL import Image, ImageFilter

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
SRC = os.environ.get(
    "PJDESK_ICON_SRC",
    "/home/ubuntu/.cursor/projects/workspace/uploads/pjdesk-icon-src_826b.png",
)
SQ = 10  # checker square, measured from the source
LIGHT_BG = (244, 246, 248, 255)  # Android / apple-touch plate


def load_rgb(path):
    im = Image.open(path).convert("RGB")
    return np.asarray(im).astype(np.int16)


def checker_phase(h, w):
    ys, xs = np.indices((h, w))
    return ((xs // SQ) + (ys // SQ)) % 2


def box_mean(img, rad):
    c = np.pad(img.astype(np.float32), ((1, 0), (1, 0)))
    ii = c.cumsum(0).cumsum(1)
    h, w = img.shape
    y0 = np.clip(np.arange(h) - rad, 0, h)
    y1 = np.clip(np.arange(h) + rad + 1, 0, h)
    x0 = np.clip(np.arange(w) - rad, 0, w)
    x1 = np.clip(np.arange(w) + rad + 1, 0, w)
    s = ii[y1[:, None], x1[None, :]] - ii[y0[:, None], x1[None, :]]
    s = s - ii[y1[:, None], x0[None, :]] + ii[y0[:, None], x0[None, :]]
    area = (y1 - y0)[:, None] * (x1 - x0)[None, :]
    return s / area


def foreground_mask(arr):
    """True where the artwork is, false on the baked-in checkerboard."""
    lum = arr.mean(axis=2)
    sat = (arr.max(axis=2) - arr.min(axis=2)).astype(np.float32)
    phase = checker_phase(*arr.shape[:2])
    dark = box_mean(np.where(phase == 0, lum, 0), 11)
    dark = dark / np.maximum(box_mean((phase == 0).astype(np.float32), 11), 1e-3)
    light = box_mean(np.where(phase == 1, lum, 0), 11)
    light = light / np.maximum(box_mean((phase == 1).astype(np.float32), 11), 1e-3)
    contrast = light - dark
    # A light checker cell sitting on the white plastic matches the background.
    # A dark checker cell covered by that plastic does not: both local phases
    # go bright and the checker contrast collapses.
    white = (sat < 30) & (contrast < 6) & (np.minimum(dark, light) > 240) & (lum > 240)
    color = sat > 25
    dark_obj = lum < 190
    bg = (sat < 18) & (contrast > 7) & (dark > 215) & (light > 230) & (lum > 215)
    return ((color | dark_obj | white) & ~bg)


def fill_holes(fg):
    """Transparent pixels that cannot reach the border are holes in the art."""
    h, w = fg.shape
    bg = ~fg
    outside = np.zeros((h, w), dtype=bool)
    outside[0, :] = bg[0, :]
    outside[-1, :] = bg[-1, :]
    outside[:, 0] = bg[:, 0]
    outside[:, -1] = bg[:, -1]
    changed = True
    while changed:
        grown = outside.copy()
        grown[1:] |= outside[:-1]
        grown[:-1] |= outside[1:]
        grown[:, 1:] |= outside[:, :-1]
        grown[:, :-1] |= outside[:, 1:]
        grown &= bg
        changed = int(grown.sum()) != int(outside.sum())
        outside = grown
    return fg | (bg & ~outside)


def largest_component(fg):
    h, w = fg.shape
    seen = np.zeros((h, w), dtype=bool)
    best = None
    best_n = 0
    ys, xs = np.where(fg)
    for y, x in zip(ys.tolist(), xs.tolist()):
        if seen[y, x]:
            continue
        stack = [(y, x)]
        seen[y, x] = True
        cells = []
        while stack:
            cy, cx = stack.pop()
            cells.append((cy, cx))
            for ny, nx in ((cy - 1, cx), (cy + 1, cx), (cy, cx - 1), (cy, cx + 1)):
                if 0 <= ny < h and 0 <= nx < w and fg[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        if len(cells) > best_n:
            best_n = len(cells)
            best = cells
    out = np.zeros((h, w), dtype=bool)
    for y, x in best:
        out[y, x] = True
    return out


def paint_holes(arr, fg, holes):
    """Copy a neighboring artwork color into holes left by the checkerboard."""
    rgb = arr.copy()
    unknown = holes.copy()
    for _ in range(24):
        if not unknown.any():
            break
        known = ~unknown & fg
        for dy, dx in ((0, 1), (0, -1), (1, 0), (-1, 0)):
            shifted = np.roll(np.roll(rgb, dy, axis=0), dx, axis=1)
            shifted_known = np.roll(np.roll(known, dy, axis=0), dx, axis=1)
            if dy == 1:
                shifted_known[0, :] = False
            elif dy == -1:
                shifted_known[-1, :] = False
            if dx == 1:
                shifted_known[:, 0] = False
            elif dx == -1:
                shifted_known[:, -1] = False
            take = unknown & shifted_known
            rgb[take] = shifted[take]
            unknown[take] = False
            known = ~unknown & fg
    return rgb


def soft_alpha(mask):
    """One-pixel edge so the cutout is not a hard stair."""
    hard = Image.fromarray(np.where(mask, np.uint8(255), np.uint8(0)))
    blurred = hard.filter(ImageFilter.GaussianBlur(0.7))
    alpha = np.asarray(blurred).astype(np.uint8)
    alpha[mask] = 255
    return alpha


def extract(arr):
    fg = foreground_mask(arr)
    filled = fill_holes(fg)
    filled = largest_component(filled)
    holes = filled & ~fg
    rgb = paint_holes(arr, filled, holes)
    alpha = soft_alpha(filled)
    out = np.zeros(arr.shape[:2] + (4,), np.uint8)
    out[..., :3] = np.clip(rgb, 0, 255).astype(np.uint8)
    out[..., 3] = alpha
    return out


def tight_square(rgba, pad_frac=0.035):
    alpha = rgba[..., 3]
    ys, xs = np.where(alpha > 16)
    y0, y1 = int(ys.min()), int(ys.max())
    x0, x1 = int(xs.min()), int(xs.max())
    side = max(x1 - x0, y1 - y0)
    pad = max(2, int(round(side * pad_frac)))
    side = side + pad * 2
    cx = (x0 + x1) / 2
    cy = (y0 + y1) / 2
    left = int(round(cx - side / 2))
    top = int(round(cy - side / 2))
    canvas = np.zeros((side, side, 4), np.uint8)
    src_x0 = max(0, left)
    src_y0 = max(0, top)
    src_x1 = min(rgba.shape[1], left + side)
    src_y1 = min(rgba.shape[0], top + side)
    dx = src_x0 - left
    dy = src_y0 - top
    canvas[dy:dy + (src_y1 - src_y0), dx:dx + (src_x1 - src_x0)] = rgba[src_y0:src_y1, src_x0:src_x1]
    return canvas


def screen_crop(master):
    """Monitor and cursor, for 16 and 24 px.

    The cable and the server box fall apart at that size. The source's white
    frame is the same color as the checkerboard, so it cannot be recovered;
    a light rim is drawn around the glass so the monitor still reads on a
    dark tray.
    """
    rgb = master[..., :3].astype(np.int16)
    alpha = master[..., 3]
    lum = rgb.mean(axis=2)
    screen = (alpha > 200) & (lum < 40)
    ys, xs = np.where(screen)
    y0, y1 = int(ys.min()), int(ys.max())
    x0, x1 = int(xs.min()), int(xs.max())
    # A few pixels of the glass edge only. The rim below is the frame.
    m = max(2, int(max(x1 - x0, y1 - y0) * 0.012))
    x0 = max(0, x0 - m)
    y0 = max(0, y0 - m)
    x1 = min(master.shape[1] - 1, x1 + m)
    y1 = min(master.shape[0] - 1, y1 + m)
    crop = master[y0:y1 + 1, x0:x1 + 1].copy()
    glass = crop[..., 3] > 128
    rim = glass.copy()
    band = max(2, crop.shape[0] // 18)
    for _ in range(band):
        grown = rim.copy()
        grown[1:] |= rim[:-1]
        grown[:-1] |= rim[1:]
        grown[:, 1:] |= rim[:, :-1]
        grown[:, :-1] |= rim[:, 1:]
        rim = grown
    frame = rim & ~glass
    crop[frame, 0] = 248
    crop[frame, 1] = 250
    crop[frame, 2] = 252
    crop[frame, 3] = 255
    return tight_square(crop, pad_frac=0.06)


def outline(rgba, color=(32, 40, 52, 255)):
    """A one-pixel rim so the white bezel still reads on a light tray."""
    alpha = rgba[..., 3]
    hard = alpha > 128
    grown = hard.copy()
    grown[1:] |= hard[:-1]
    grown[:-1] |= hard[1:]
    grown[:, 1:] |= hard[:, :-1]
    grown[:, :-1] |= hard[:, 1:]
    rim = grown & ~hard
    out = rgba.copy()
    out[rim, 0] = color[0]
    out[rim, 1] = color[1]
    out[rim, 2] = color[2]
    out[rim, 3] = 255
    return out


def resize(arr, size):
    im = Image.fromarray(arr, "RGBA").resize((size, size), Image.LANCZOS)
    return np.asarray(im)


def save_png(arr, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    Image.fromarray(arr, "RGBA").save(path, optimize=True)


def save_rgba(arr, path):
    open(path, "wb").write(np.ascontiguousarray(arr).tobytes())


def _png_bytes(arr):
    buf = io.BytesIO()
    Image.fromarray(arr, "RGBA").save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def save_ico(path, images):
    """Multi-size icon. Each entry is a PNG, which Windows and windres both read."""
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
    return np.concatenate([out.astype(np.uint8), np.full(arr.shape[:2] + (1,), 255, np.uint8)], axis=2)


def adaptive_foreground(master, size):
    """Artwork inside the adaptive-icon safe circle (about 66 of 108)."""
    canvas = np.zeros((size, size, 4), np.uint8)
    inner = int(round(size * 0.62))
    icon = resize(master, inner)
    off = (size - inner) // 2
    canvas[off:off + inner, off:off + inner] = icon
    return canvas


def monochrome(foreground):
    """One white shape Android can tint.

    The cursor is knocked out of the glass, so it stays visible when the
    whole icon is a single color. The server box and the cable stay filled.
    """
    alpha = foreground[..., 3].astype(np.int16)
    rgb = foreground[..., :3].astype(np.int16)
    lum = rgb.mean(axis=2)
    sat = rgb.max(axis=2) - rgb.min(axis=2)
    shape = alpha > 160
    bright = shape & (lum > 200) & (sat < 50)
    dark = (alpha > 160) & (lum < 40)
    near_dark = box_mean(dark.astype(np.float32), max(2, foreground.shape[0] // 40)) > 0.3
    knockout = bright & near_dark
    out = np.zeros_like(foreground)
    out[..., 0:3] = 255
    out[..., 3] = np.where(shape & ~knockout, np.uint8(255), np.uint8(0))
    return out


def write_android(master):
    res = os.path.join(ROOT, "android", "app", "src", "main", "res")
    fg_sizes = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}
    legacy = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
    for density, size in fg_sizes.items():
        fg = adaptive_foreground(master, size)
        save_png(fg, os.path.join(res, f"mipmap-{density}", "ic_launcher_foreground.png"))
        save_png(monochrome(fg), os.path.join(res, f"mipmap-{density}", "ic_launcher_monochrome.png"))
    for density, size in legacy.items():
        icon = on_plate(resize(master, size), LIGHT_BG)
        save_png(icon, os.path.join(res, f"mipmap-{density}", "ic_launcher.png"))
    # In-app mark (the lock screen). Transparent, so it sits on either theme.
    # Lock screen and any in-app Image. A light plate keeps the black glass visible
    # on the dark theme; the launcher mask uses the same plate as its background.
    save_png(on_plate(resize(master, 192), LIGHT_BG), os.path.join(res, "drawable-nodpi", "ic_launcher.png"))
    values = os.path.join(res, "values", "ic_launcher_background.xml")
    open(values, "w").write(
        """<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#F4F6F8</color>
</resources>
"""
    )
    anydpi = os.path.join(res, "mipmap-anydpi-v26")
    os.makedirs(anydpi, exist_ok=True)
    open(os.path.join(anydpi, "ic_launcher.xml"), "w").write(
        """<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
    <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>
</adaptive-icon>
"""
    )


def write_windows(master, tray):
    client = os.path.join(ROOT, "crates", "pjdesk", "assets")
    server = os.path.join(ROOT, "crates", "pjdesk-server", "assets")
    os.makedirs(client, exist_ok=True)
    os.makedirs(server, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    full = {s: resize(master, s) for s in sizes}
    small = {16: outline(resize(tray, 16)), 24: outline(resize(tray, 24))}
    images = [small.get(s, full[s]) for s in sizes]
    for folder, ico_name, png_name, rgba_prefix in (
        (client, "pjdesk.ico", "icon_256.png", "icon"),
        (server, "pjdesk-server.ico", "icon_server_256.png", "icon_server"),
    ):
        save_ico(os.path.join(folder, ico_name), images)
        save_png(full[256], os.path.join(folder, png_name))
        save_rgba(full[256], os.path.join(folder, f"{rgba_prefix}_256.rgba"))
        # Tray source is 32 px. 16 and 24 are the same simplified mark, scaled.
        save_rgba(outline(resize(tray, 32)), os.path.join(folder, f"{rgba_prefix}_32.rgba"))


def write_site(master, tray):
    public = os.path.join(ROOT, "cloud", "public")
    logo = resize(master, 128)
    save_png(logo, os.path.join(public, "logo.png"))
    save_png(on_plate(resize(master, 180), LIGHT_BG), os.path.join(public, "apple-touch-icon.png"))
    save_png(outline(resize(tray, 32)), os.path.join(public, "favicon-32.png"))
    save_ico(
        os.path.join(public, "favicon.ico"),
        [outline(resize(tray, 16)), outline(resize(tray, 32)), outline(resize(tray, 48))],
    )
    readme = resize(master, 256)
    save_png(readme, os.path.join(ROOT, "docs", "pjdesk-icon.png"))


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else SRC
    arr = load_rgb(src)
    if arr.shape[0] != 1024 or arr.shape[1] != 1024:
        raise SystemExit(f"expected 1024x1024, got {arr.shape[1]}x{arr.shape[0]}")
    extracted = extract(arr)
    master = tight_square(extracted)
    tray = screen_crop(master)
    # Keep a square master at 1024 so later sizes stay sharp.
    master = resize(master, 1024)
    tray = resize(tray, 1024)
    write_windows(master, tray)
    write_android(master)
    write_site(master, tray)
    Image.fromarray(master, "RGBA").save("/tmp/pjdesk-master.png")
    Image.fromarray(tray, "RGBA").save("/tmp/pjdesk-tray-master.png")
    for n, src_arr in ((16, tray), (24, tray), (32, master), (48, master)):
        Image.fromarray(outline(resize(src_arr if n < 32 else master, n)), "RGBA").resize((n * 8, n * 8), Image.NEAREST).save(f"/tmp/pjdesk-{n}.png")
    print("wrote icons from", src)


if __name__ == "__main__":
    main()
