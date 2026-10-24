#!/usr/bin/env python3
"""Build the PJDesk icon from the supplied artwork.

The source is a 1024x1024 PNG. Its checkerboard is painted into the pixels,
not stored as an alpha channel. The light squares are almost the same color
as the white monitor bezel, stand, and server box, so a brightness cutoff
deletes those parts. This script flood-fills only the checkerboard that
touches the image edge: a pixel counts as checker when a neighborhood holds
both the dark squares and the light squares. Solid white artwork has no dark
squares, so the bezel, stand, and server stay. The edge is antialiased, and
a soft dark halo keeps the white parts readable on a dark title bar.
"""
import io
import os
import struct
import sys

import numpy as np
from PIL import Image, ImageFilter

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
SRC_CANDIDATES = [
    os.environ.get("PJDESK_ICON_SRC", ""),
    "/workspace/pjdesk-icon-src.png",
    "/home/ubuntu/.cursor/projects/workspace/uploads/pjdesk-icon-src_826b.png",
]
LIGHT_BG = (244, 246, 248, 255)  # Android / apple-touch plate
DARK_BG = (0x1E, 0x1E, 0x1E)
HALO = (32, 36, 44)


def source_path(argv):
    if len(argv) > 1:
        return argv[1]
    for path in SRC_CANDIDATES:
        if path and os.path.isfile(path):
            return path
    raise SystemExit("icon source PNG not found")


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
    return s / np.maximum(area, 1)


def checker_mask(rgb):
    """True on checkerboard pixels that belong to the backdrop.

    Dark squares are about 240 and light squares about 254, in a 10px grid.
    The white plastic is also about 254, but a window on it does not contain
    the dark squares, so it is not the backdrop.
    """
    lum = rgb.mean(axis=2)
    sat = rgb.max(axis=2) - rgb.min(axis=2)
    neutral = sat < 28
    near_dark = neutral & (lum >= 230) & (lum <= 247)
    near_light = neutral & (lum >= 248)
    frac_d = box_mean(near_dark.astype(np.float32), 14)
    frac_l = box_mean(near_light.astype(np.float32), 14)
    both = (frac_d > 0.22) & (frac_l > 0.22)
    return both & (near_dark | near_light), frac_d, neutral, lum


def flood_from_edges(cand):
    bg = np.zeros(cand.shape, dtype=bool)
    bg[0, :] = cand[0, :]
    bg[-1, :] = cand[-1, :]
    bg[:, 0] |= cand[:, 0]
    bg[:, -1] |= cand[:, -1]
    while True:
        grown = bg.copy()
        grown[1:] |= bg[:-1]
        grown[:-1] |= bg[1:]
        grown[:, 1:] |= bg[:, :-1]
        grown[:, :-1] |= bg[:, 1:]
        grown &= cand
        if int(grown.sum()) == int(bg.sum()):
            return bg
        bg = grown


def largest_component(fg):
    h, w = fg.shape
    seen = np.zeros((h, w), dtype=bool)
    best = []
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
        if len(cells) > len(best):
            best = cells
    out = np.zeros((h, w), dtype=bool)
    for y, x in best:
        out[y, x] = True
    return out


def extract(rgb):
    cand, frac_d, neutral, lum = checker_mask(rgb)
    bg = flood_from_edges(cand)
    # Cell edges that missed the color bands still sit in a checker neighborhood.
    # Solid white has almost no dark squares nearby, so this does not eat it.
    bg |= (frac_d > 0.30) & neutral & (lum > 226)
    mask = largest_component(~bg)

    hard = mask
    grown = hard.copy()
    grown[1:] |= hard[:-1]
    grown[:-1] |= hard[1:]
    grown[:, 1:] |= hard[:, :-1]
    grown[:, :-1] |= hard[:, 1:]
    fringe = grown & ~hard
    acc = np.zeros_like(rgb)
    cnt = np.zeros(lum.shape, np.float32)
    for dy, dx in ((0, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)):
        shifted = np.roll(np.roll(rgb, dy, 0), dx, 1)
        known = np.roll(np.roll(hard, dy, 0), dx, 1)
        if dy > 0:
            known[:dy, :] = False
        elif dy < 0:
            known[dy:, :] = False
        if dx > 0:
            known[:, :dx] = False
        elif dx < 0:
            known[:, dx:] = False
        acc += shifted * known[..., None]
        cnt += known
    color = np.where(cnt[..., None] > 0, acc / np.maximum(cnt[..., None], 1), rgb)
    alpha = np.zeros(lum.shape, np.float32)
    alpha[hard] = 255
    alpha[fringe] = np.clip(cnt[fringe] / 5.0 * 255.0, 40, 210)
    rgba = np.zeros(rgb.shape[:2] + (4,), np.uint8)
    rgba[..., :3] = np.clip(np.where(fringe[..., None], color, rgb), 0, 255).astype(np.uint8)
    rgba[..., 3] = alpha.astype(np.uint8)
    return rgba


def tight_square(rgba, pad_frac=0.07):
    ys, xs = np.where(rgba[..., 3] > 20)
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
    sx0, sy0 = max(0, left), max(0, top)
    sx1, sy1 = min(rgba.shape[1], left + side), min(rgba.shape[0], top + side)
    canvas[sy0 - top : sy0 - top + (sy1 - sy0), sx0 - left : sx0 - left + (sx1 - sx0)] = rgba[
        sy0:sy1, sx0:sx1
    ]
    return canvas


def soft_halo(rgba):
    """A faint dark edge outside the artwork so white plastic reads on dark and light."""
    blurred = np.asarray(Image.fromarray(rgba[..., 3]).filter(ImageFilter.GaussianBlur(2.2))).astype(
        np.float32
    )
    shadow_a = np.clip(blurred * 0.55, 0, 160)
    art_a = rgba[..., 3].astype(np.float32) / 255.0
    out_a = art_a + (shadow_a / 255.0) * (1.0 - art_a)
    art_rgb = rgba[..., :3].astype(np.float32)
    shadow_rgb = np.array(HALO, np.float32)
    # Premultiplied over: artwork covers the halo, the halo tints only the outside.
    prem = art_rgb * art_a[..., None] + shadow_rgb * (shadow_a / 255.0)[..., None] * (1.0 - art_a)[..., None]
    rgb = prem / np.maximum(out_a[..., None], 1e-4)
    out = np.zeros_like(rgba)
    out[..., :3] = np.clip(rgb, 0, 255).astype(np.uint8)
    out[..., 3] = np.clip(out_a * 255.0, 0, 255).astype(np.uint8)
    out[rgba[..., 3] == 255] = rgba[rgba[..., 3] == 255]
    return out


def outline(rgba, color=(32, 40, 52, 230)):
    """A one-pixel rim so the white bezel still reads at tray size on a light bar."""
    alpha = rgba[..., 3]
    hard = alpha > 128
    grown = hard.copy()
    grown[1:] |= hard[:-1]
    grown[:-1] |= hard[1:]
    grown[:, 1:] |= hard[:, :-1]
    grown[:, :-1] |= hard[:, 1:]
    rim = grown & ~hard & (alpha < 80)
    out = rgba.copy()
    out[rim, 0] = color[0]
    out[rim, 1] = color[1]
    out[rim, 2] = color[2]
    out[rim, 3] = color[3]
    return out


def resize(arr, size):
    return np.asarray(Image.fromarray(arr, "RGBA").resize((size, size), Image.LANCZOS))


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
    canvas[off : off + inner, off : off + inner] = icon
    return canvas


def monochrome(foreground):
    """One white shape Android can tint.

    The cursor is knocked out of the glass so it stays a hole when the icon
    is a single color. The bezel, server box, and cable stay filled.
    """
    alpha = foreground[..., 3].astype(np.int16)
    rgb = foreground[..., :3].astype(np.int16)
    lum = rgb.mean(axis=2)
    sat = rgb.max(axis=2) - rgb.min(axis=2)
    shape = alpha > 160
    bright = shape & (lum > 200) & (sat < 50)
    dark = (alpha > 160) & (lum < 40)
    # The cursor sits in the glass. The bezel only touches the glass on one side.
    dark_frac = box_mean(dark.astype(np.float32), max(2, foreground.shape[0] // 28))
    knockout = bright & (dark_frac > 0.55)
    out = np.zeros_like(foreground)
    out[..., 0:3] = 255
    out[..., 3] = np.where(shape & ~knockout, np.uint8(255), np.uint8(0))
    return out


def verify(master):
    """The bezel, stand, and server must still be white on black and on white."""
    for bg, name in ((DARK_BG, "black"), ((255, 255, 255), "white")):
        plate = on_plate(master, bg + (255,))
        if not np.array_equal(plate[0, 0, :3], np.array(bg)) or not np.array_equal(plate[-1, -1, :3], np.array(bg)):
            raise SystemExit(f"{name} composite corners are not the flat background")
        # No leftover checker: the outer margin is one flat color.
        margin = np.concatenate([plate[:8].reshape(-1, 4), plate[-8:].reshape(-1, 4)])
        if margin[:, :3].std() > 1.5:
            raise SystemExit(f"{name} composite still has a pattern in the margin")
    black = on_plate(resize(master, 256), DARK_BG + (255,))
    lum = black[..., :3].mean(axis=2)
    light_edge = 0
    dark_edge = 0
    rows = 0
    for y in range(black.shape[0]):
        xs = np.where(np.abs(lum[y] - DARK_BG[0]) > 12)[0]
        if len(xs) < 8:
            continue
        rows += 1
        sample = lum[y, xs[0] : xs[0] + 3].mean()
        if sample > 180:
            light_edge += 1
        elif sample < 80:
            dark_edge += 1
    if light_edge < 150 or dark_edge > 30:
        raise SystemExit(
            f"white bezel is missing on black (light edge rows {light_edge}, dark {dark_edge}, of {rows})"
        )
    white = ((lum > 230) & (np.abs(black[..., 0] - black[..., 2]) < 20)).sum()
    glass = (lum < 25).sum()
    blue = ((black[..., 2] > 150) & (black[..., 2] > black[..., 0] + 40)).sum()
    if white < 6000 or glass < 8000 or blue < 800:
        raise SystemExit(f"artwork incomplete on black: white {white} glass {glass} blue {blue}")
    return light_edge, dark_edge, int(white), int(glass), int(blue)


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


def write_windows(master):
    client = os.path.join(ROOT, "crates", "pjdesk", "assets")
    server = os.path.join(ROOT, "crates", "pjdesk-server", "assets")
    os.makedirs(client, exist_ok=True)
    os.makedirs(server, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    full = {s: resize(master, s) for s in sizes}
    # 16 and 24 gain a hard rim. The soft halo is only a fraction of a pixel there.
    images = [outline(full[s]) if s <= 24 else full[s] for s in sizes]
    tray = outline(full[32])
    for folder, ico_name, png_name, rgba_prefix in (
        (client, "pjdesk.ico", "icon_256.png", "icon"),
        (server, "pjdesk-server.ico", "icon_server_256.png", "icon_server"),
    ):
        save_ico(os.path.join(folder, ico_name), images)
        save_png(full[256], os.path.join(folder, png_name))
        save_rgba(full[256], os.path.join(folder, f"{rgba_prefix}_256.rgba"))
        save_rgba(tray, os.path.join(folder, f"{rgba_prefix}_32.rgba"))


def write_site(master):
    public = os.path.join(ROOT, "cloud", "public")
    logo = resize(master, 128)
    save_png(logo, os.path.join(public, "logo.png"))
    save_png(on_plate(resize(master, 180), LIGHT_BG), os.path.join(public, "apple-touch-icon.png"))
    save_png(outline(resize(master, 32)), os.path.join(public, "favicon-32.png"))
    save_ico(
        os.path.join(public, "favicon.ico"),
        [outline(resize(master, 16)), outline(resize(master, 32)), outline(resize(master, 48))],
    )
    save_png(resize(master, 256), os.path.join(ROOT, "docs", "pjdesk-icon.png"))


def write_previews(master):
    out_dir = "/opt/cursor/artifacts/pjdesk-icon-0625"
    os.makedirs(out_dir, exist_ok=True)
    black = on_plate(resize(master, 768), DARK_BG + (255,))
    white = on_plate(resize(master, 768), (255, 255, 255, 255))
    save_png(black, os.path.join(out_dir, "on-black.png"))
    save_png(white, os.path.join(out_dir, "on-white.png"))
    bar = Image.new("RGBA", (480, 72), DARK_BG + (255,))
    icon = Image.fromarray(resize(master, 40), "RGBA")
    bar.paste(icon, (16, 16), icon)
    bar.save(os.path.join(out_dir, "titlebar-dark.png"))
    for n in (16, 24, 32):
        small = outline(resize(master, n))
        Image.fromarray(on_plate(small, DARK_BG + (255,)), "RGBA").resize((n * 12, n * 12), Image.NEAREST).save(
            os.path.join(out_dir, f"tray-{n}-dark.png")
        )
        Image.fromarray(on_plate(small, (255, 255, 255, 255)), "RGBA").resize((n * 12, n * 12), Image.NEAREST).save(
            os.path.join(out_dir, f"tray-{n}-light.png")
        )


def main():
    src = source_path(sys.argv)
    rgb = np.asarray(Image.open(src).convert("RGB")).astype(np.float32)
    if rgb.shape[0] != 1024 or rgb.shape[1] != 1024:
        raise SystemExit(f"expected 1024x1024, got {rgb.shape[1]}x{rgb.shape[0]}")
    extracted = extract(rgb)
    master = resize(soft_halo(tight_square(extracted)), 1024)
    stats = verify(master)
    write_windows(master)
    write_android(master)
    write_site(master)
    write_previews(master)
    print("wrote icons from", src, "bezel-rows", stats[0], "white", stats[2], "glass", stats[3], "blue", stats[4])


if __name__ == "__main__":
    main()
