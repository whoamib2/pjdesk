#!/usr/bin/env python3
"""Draw the PJDesk icons as vectors and rasterize every size from those SVGs.

The supplied artwork is a monitor, cursor, cable, stand, and server painted on
a checkerboard. Cutting that checkerboard out left a sawtooth fringe and ate
the stand, because the white plastic is the same color as the light squares.
These marks are redrawn: crisp curves, a transparent background, and a thin
dark outline so the white reads on a dark title bar.

The client keeps the monitor. PJDesk Server has its own mark in the same
palette: a large server box in front, with a small monitor and cursor behind
it. Sizes 16 and 24 use a bolder drawing of the same shapes so the tray stays
legible.
"""
import io
import math
import os
import struct

import cairosvg
import numpy as np
from PIL import Image, ImageDraw

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BLUE = "#0AB4F5"
WHITE = "#FFFFFF"
GREY = "#E0E4EA"
SCREEN = "#000000"
INK = "#080A10"
LIGHT_BG = (244, 246, 248, 255)
DARK_BG = (0x1E, 0x1E, 0x1E)
SIZES = (16, 24, 32, 48, 64, 128, 256)


def rounded_poly(points, radius):
    """Closed polygon with quadratic corners. points are (x, y) in order."""
    n = len(points)
    curves = []
    for i in range(n):
        p = points[i]
        a = points[(i - 1) % n]
        b = points[(i + 1) % n]
        va = (a[0] - p[0], a[1] - p[1])
        vb = (b[0] - p[0], b[1] - p[1])
        la = math.hypot(*va) or 1.0
        lb = math.hypot(*vb) or 1.0
        r = min(radius, la * 0.46, lb * 0.46)
        p1 = (p[0] + va[0] / la * r, p[1] + va[1] / la * r)
        p2 = (p[0] + vb[0] / lb * r, p[1] + vb[1] / lb * r)
        curves.append((p1, p, p2))
    start = curves[0][0]
    parts = [f"M {start[0]:.2f} {start[1]:.2f}"]
    for i, (p1, ctrl, p2) in enumerate(curves):
        parts.append(f"Q {ctrl[0]:.2f} {ctrl[1]:.2f} {p2[0]:.2f} {p2[1]:.2f}")
        nxt = curves[(i + 1) % n][0]
        parts.append(f"L {nxt[0]:.2f} {nxt[1]:.2f}")
    parts.append("Z")
    return " ".join(parts)


def poly(points, radius, fill, stroke):
    d = rounded_poly(points, radius)
    rim = (
        f'<path d="{d}" fill="{INK}" stroke="{INK}" stroke-width="{stroke:.2f}" '
        f'stroke-linejoin="round" stroke-linecap="round"/>'
    )
    return rim + f'<path d="{d}" fill="{fill}"/>'


def rect(x, y, w, h, rx, fill, stroke):
    attrs = f'x="{x:.2f}" y="{y:.2f}" width="{w:.2f}" height="{h:.2f}" rx="{rx:.2f}"'
    return (
        f'<rect {attrs} fill="{INK}" stroke="{INK}" stroke-width="{stroke:.2f}"/>'
        f'<rect {attrs} fill="{fill}"/>'
    )


def circle(cx, cy, r, fill, stroke):
    attrs = f'cx="{cx:.2f}" cy="{cy:.2f}" r="{r:.2f}"'
    return (
        f'<circle {attrs} fill="{INK}" stroke="{INK}" stroke-width="{stroke:.2f}"/>'
        f'<circle {attrs} fill="{fill}"/>'
    )


def wrap(body):
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" '
        'shape-rendering="geometricPrecision">\n'
        f"{body}\n</svg>\n"
    )


def client_body(stroke, bold):
    """Monitor in front. `bold` thickens the parts that disappear at 16 px."""
    bezel = 24 if bold else 15
    radius = 18
    # Front face, slight 3/4: the top falls to the right and that side is shorter.
    front = [(44, 30), (198, 52), (200, 156), (42, 178)]
    screen = [
        (front[0][0] + bezel, front[0][1] + bezel),
        (front[1][0] - bezel + 4, front[1][1] + bezel - 4),
        (front[2][0] - bezel + 4, front[2][1] - bezel + 8),
        (front[3][0] + bezel, front[3][1] - bezel + 10),
    ]
    side = [(30, 40), (44, 30), (42, 178), (28, 190)]
    # Arrow points up-left. The cable leaves the right-hand corner.
    if bold:
        cursor = [(84, 58), (172, 92), (162, 114), (130, 106), (124, 142), (108, 96)]
        cable_w = 20.0
        cable = "M 168 98 C 204 108 226 132 214 158"
    else:
        cursor = [(90, 62), (166, 96), (158, 114), (130, 108), (126, 140), (110, 100)]
        cable_w = 15.0
        # Leaves the cursor, crosses the bezel, and ends in the server box.
        cable = "M 152 100 C 186 108 214 132 198 168"
    # Neck under the bezel, then a wide foot. The cable stays to the right of this.
    stripe = rounded_poly([(102, 174), (132, 166), (136, 180), (106, 188)], 3)
    server = (168, 164, 66, 68, 14) if not bold else (158, 150, 78, 80, 16)
    parts = [
        poly(side, 7, GREY, stroke),
        poly([(98, 170), (136, 160), (144, 198), (162, 216), (116, 232), (68, 220), (78, 198)], 16, WHITE, stroke),
        poly(front, radius, WHITE, stroke),
        f'<path d="{stripe}" fill="{BLUE}"/>',
        poly(screen, 11, SCREEN, 0),
        poly(cursor, 2.0, WHITE, 0 if bold else max(stroke * 0.35, 1.4)),
        f'<path d="{cable}" fill="none" stroke="{INK}" stroke-width="{cable_w + stroke:.2f}" '
        f'stroke-linecap="round"/>'
        f'<path d="{cable}" fill="none" stroke="{BLUE}" stroke-width="{cable_w:.2f}" '
        f'stroke-linecap="round"/>',
    ]
    x, y, w, h, rx = server
    parts.append(rect(x, y, w, h, rx, WHITE, stroke))
    bar_w = w * 0.64
    bar_x = x + w * 0.18
    bar_h = 11 if bold else 8
    gap = 7 if bold else 6
    bar_y = y + h * 0.18
    parts.append(rect(bar_x, bar_y, bar_w, bar_h, 3, BLUE, 0))
    parts.append(rect(bar_x, bar_y + bar_h + gap, bar_w, bar_h, 3, BLUE, 0))
    parts.append(circle(bar_x + bar_h * 0.55, bar_y + 2 * (bar_h + gap) + bar_h * 0.45, bar_h * 0.42, BLUE, 0))
    return "".join(parts)


def server_body(stroke, bold):
    """Large server box in front of a small monitor."""
    if bold:
        # The tray size. The box, two bars, and the dot have to survive 16 px.
        box = (32, 36, 192, 184, 28)
        cable = "M 196 48 C 214 36 228 52 210 64"
        cable_w = max(16.0, stroke * 0.5)
        parts = [
            f'<path d="{cable}" fill="none" stroke="{INK}" stroke-width="{cable_w + stroke:.2f}" '
            f'stroke-linecap="round"/>'
            f'<path d="{cable}" fill="none" stroke="{BLUE}" stroke-width="{cable_w:.2f}" '
            f'stroke-linecap="round"/>'
        ]
        x, y, w, h, rx = box
        parts.append(rect(x, y, w, h, rx, WHITE, stroke))
        bar_w = 132
        bar_x = x + 30
        bar_h = 28
        parts.append(rect(bar_x, y + 32, bar_w, bar_h, 8, BLUE, 0))
        parts.append(rect(bar_x, y + 76, bar_w, bar_h, 8, BLUE, 0))
        parts.append(circle(bar_x + 14, y + 136, 12, BLUE, 0))
        return "".join(parts)

    # Small monitor behind, same perspective as the client.
    front = [(22, 24), (132, 42), (134, 112), (20, 128)]
    screen = [(38, 40), (116, 54), (118, 98), (36, 112)]
    side = [(12, 32), (22, 24), (20, 128), (10, 138)]
    cursor = [(52, 52), (104, 74), (98, 88), (78, 84), (74, 104), (64, 78)]
    cable = "M 102 78 C 150 88 168 96 160 118"
    cable_w = 10.0
    parts = [
        poly(side, 6, GREY, stroke),
        poly(front, 12, WHITE, stroke),
        poly(screen, 8, SCREEN, 0.01),
        poly(cursor, 1.8, WHITE, max(stroke * 0.4, 1.2)),
        f'<path d="{cable}" fill="none" stroke="{INK}" stroke-width="{cable_w + stroke:.2f}" '
        f'stroke-linecap="round"/>'
        f'<path d="{cable}" fill="none" stroke="{BLUE}" stroke-width="{cable_w:.2f}" '
        f'stroke-linecap="round"/>',
    ]
    x, y, w, h, rx = (78, 100, 156, 128, 22)
    parts.append(rect(x, y, w, h, rx, WHITE, stroke))
    bar_w = 108
    bar_x = x + 24
    bar_h = 18
    parts.append(rect(bar_x, y + 22, bar_w, bar_h, 5, BLUE, 0))
    parts.append(rect(bar_x, y + 52, bar_w, bar_h, 5, BLUE, 0))
    parts.append(circle(bar_x + 10, y + 90, 8, BLUE, 0))
    return "".join(parts)


def monochrome(foreground):
    """White silhouette. The cursor is the white island that does not touch the outside."""
    alpha = foreground[..., 3]
    rgb = foreground[..., :3].astype(np.int16)
    lum = rgb.mean(axis=2)
    sat = rgb.max(axis=2) - rgb.min(axis=2)
    shape = alpha > 140
    bright = shape & (lum > 200) & (sat < 45)
    h, w = bright.shape
    seen = np.zeros((h, w), dtype=bool)
    knockout = np.zeros((h, w), dtype=bool)
    ys, xs = np.where(bright)
    for y, x in zip(ys.tolist(), xs.tolist()):
        if seen[y, x]:
            continue
        stack = [(y, x)]
        seen[y, x] = True
        cells = []
        touches_outside = False
        while stack:
            cy, cx = stack.pop()
            cells.append((cy, cx))
            for ny, nx in ((cy - 1, cx), (cy + 1, cx), (cy, cx - 1), (cy, cx + 1)):
                if not (0 <= ny < h and 0 <= nx < w):
                    touches_outside = True
                    continue
                if alpha[ny, nx] < 40:
                    touches_outside = True
                if bright[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        if not touches_outside and len(cells) > 12:
            for cy, cx in cells:
                knockout[cy, cx] = True
    out = np.zeros_like(foreground)
    out[..., 0:3] = 255
    out[..., 3] = np.where(shape & ~knockout, np.uint8(255), np.uint8(0))
    return out


def outline_px(size):
    if size <= 16:
        return 1.15
    if size <= 24:
        return 1.3
    if size <= 32:
        return 1.45
    if size <= 48:
        return 1.7
    return 2.0


def svg_for(kind, size):
    # Stroke width is in viewBox units. Half of it sits outside the fill.
    stroke = 2.0 * outline_px(size) * (256.0 / size)
    bold = size <= 24
    body = client_body(stroke, bold) if kind == "client" else server_body(stroke, bold)
    return wrap(body)


def raster(svg, size):
    scale = 4
    png = cairosvg.svg2png(
        bytestring=svg.encode("utf-8"),
        output_width=size * scale,
        output_height=size * scale,
    )
    im = Image.open(io.BytesIO(png)).convert("RGBA")
    im = im.resize((size, size), Image.Resampling.BOX)
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


def resize(arr, size):
    return np.asarray(Image.fromarray(arr, "RGBA").resize((size, size), Image.Resampling.BOX))


def edge_jitter(arr):
    """Second difference of the left silhouette. A torn cutout jumps by several pixels."""
    xs = []
    for y in range(arr.shape[0]):
        hit = np.where(arr[y, :, 3] > 128)[0]
        if len(hit):
            xs.append(int(hit[0]))
    if len(xs) < 6:
        return 0
    n = len(xs)
    sl = np.array(xs[n // 6 : n // 2])
    if len(sl) < 4:
        return 0
    return int(np.max(np.abs(np.diff(sl, n=2))))


def verify(images):
    client = images["client"][256]
    server = images["server"][256]
    if np.array_equal(client, server):
        raise SystemExit("server icon matches the client")
    for name, im in (("client", client), ("server", server)):
        if im[0, 0, 3] != 0 or im[-1, -1, 3] != 0:
            raise SystemExit(f"{name} corner is not transparent")
        jitter = edge_jitter(im)
        if jitter > 2:
            raise SystemExit(f"{name} silhouette is jagged (edge step {jitter})")
        for bg, label in ((DARK_BG, "black"), ((255, 255, 255), "white")):
            plate = on_plate(im, bg + (255,))
            if plate[..., :3].reshape(-1, 3)[:8].std() > 1.5:
                raise SystemExit(f"{name} on {label} still has a fringe in the margin")
    black = on_plate(client, DARK_BG + (255,))
    lum = black[..., :3].mean(axis=2)
    white = int(((lum > 220) & (np.abs(black[..., 0] - black[..., 2]) < 18)).sum())
    glass = int((lum < 20).sum())
    blue = int(((black[..., 2] > 160) & (black[..., 2] > black[..., 0] + 40)).sum())
    if white < 8000 or glass < 4000 or blue < 400:
        raise SystemExit(f"client artwork incomplete: white {white} glass {glass} blue {blue}")
    sblack = on_plate(server, DARK_BG + (255,))
    slum = sblack[..., :3].mean(axis=2)
    swhite = int(((slum > 220) & (np.abs(sblack[..., 0] - sblack[..., 2]) < 18)).sum())
    sblue = int(((sblack[..., 2] > 160) & (sblack[..., 2] > sblack[..., 0] + 40)).sum())
    if swhite < 10000 or sblue < 800:
        raise SystemExit(f"server artwork incomplete: white {swhite} blue {sblue}")
    # 16 px must still show white and blue. Corners of a 16 px mark step by a pixel.
    for kind in ("client", "server"):
        small = images[kind][16]
        if edge_jitter(small) > 4:
            raise SystemExit(f"{kind} 16 px silhouette is jagged ({edge_jitter(small)})")
        plate = on_plate(small, DARK_BG + (255,))
        lum = plate[..., :3].mean(axis=2)
        if int((lum > 200).sum()) < 20 or int((plate[..., 2] > 140).sum()) < 4:
            raise SystemExit(f"{kind} 16 px lost the white or the blue")
    return white, glass, blue, swhite, sblue


def render_all():
    images = {"client": {}, "server": {}}
    for kind in ("client", "server"):
        for size in SIZES:
            images[kind][size] = raster(svg_for(kind, size), size)
    return images


def write_svgs():
    docs = os.path.join(ROOT, "docs")
    os.makedirs(docs, exist_ok=True)
    open(os.path.join(docs, "pjdesk-icon.svg"), "w").write(svg_for("client", 256))
    open(os.path.join(docs, "pjdesk-server-icon.svg"), "w").write(svg_for("server", 256))


def write_windows(images):
    client = os.path.join(ROOT, "crates", "pjdesk", "assets")
    server = os.path.join(ROOT, "crates", "pjdesk-server", "assets")
    for kind, folder, ico_name, png_name, rgba_prefix in (
        ("client", client, "pjdesk.ico", "icon_256.png", "icon"),
        ("server", server, "pjdesk-server.ico", "icon_server_256.png", "icon_server"),
    ):
        os.makedirs(folder, exist_ok=True)
        frames = [images[kind][s] for s in SIZES]
        save_ico(os.path.join(folder, ico_name), frames)
        save_png(images[kind][256], os.path.join(folder, png_name))
        save_rgba(images[kind][256], os.path.join(folder, f"{rgba_prefix}_256.rgba"))
        save_rgba(images[kind][32], os.path.join(folder, f"{rgba_prefix}_32.rgba"))


def write_android(images):
    res = os.path.join(ROOT, "android", "app", "src", "main", "res")
    master = images["client"][256]
    fg_sizes = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}
    legacy = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
    for density, size in fg_sizes.items():
        canvas = np.zeros((size, size, 4), np.uint8)
        inner = int(round(size * 0.62))
        icon = resize(master, inner)
        off = (size - inner) // 2
        canvas[off : off + inner, off : off + inner] = icon
        save_png(canvas, os.path.join(res, f"mipmap-{density}", "ic_launcher_foreground.png"))
        mono = np.zeros((size, size, 4), np.uint8)
        m = monochrome(resize(master, inner))
        mono[off : off + inner, off : off + inner] = m
        mono[..., 0:3] = np.where(mono[..., 3:4] > 0, 255, 0)
        save_png(mono, os.path.join(res, f"mipmap-{density}", "ic_launcher_monochrome.png"))
    for density, size in legacy.items():
        save_png(on_plate(resize(master, size), LIGHT_BG), os.path.join(res, f"mipmap-{density}", "ic_launcher.png"))
    save_png(on_plate(resize(master, 192), LIGHT_BG), os.path.join(res, "drawable-nodpi", "ic_launcher.png"))


def write_site(images):
    public = os.path.join(ROOT, "cloud", "public")
    master = images["client"][256]
    save_png(resize(master, 128), os.path.join(public, "logo.png"))
    save_png(on_plate(resize(master, 180), LIGHT_BG), os.path.join(public, "apple-touch-icon.png"))
    save_png(images["client"][32], os.path.join(public, "favicon-32.png"))
    save_ico(
        os.path.join(public, "favicon.ico"),
        [images["client"][16], images["client"][32], images["client"][48]],
    )
    save_png(master, os.path.join(ROOT, "docs", "pjdesk-icon.png"))
    save_png(images["server"][256], os.path.join(ROOT, "docs", "pjdesk-server-icon.png"))


def _caption(draw, text, x, y):
    draw.text((x, y), text, fill=(180, 186, 196, 255))


def write_previews(images):
    out_dir = "/opt/cursor/artifacts/pjdesk-icon-vector"
    os.makedirs(out_dir, exist_ok=True)
    gap = 48
    side = 640

    def pair(bg, path):
        a = on_plate(resize(images["client"][256], side), bg + (255,))
        b = on_plate(resize(images["server"][256], side), bg + (255,))
        canvas = Image.new("RGBA", (side * 2 + gap, side + 36), bg + (255,))
        canvas.paste(Image.fromarray(a, "RGBA"), (0, 36))
        canvas.paste(Image.fromarray(b, "RGBA"), (side + gap, 36))
        draw = ImageDraw.Draw(canvas)
        fill = (210, 214, 220, 255) if bg[0] < 128 else (40, 44, 52, 255)
        draw.text((16, 10), "Client", fill=fill)
        draw.text((side + gap + 16, 10), "Server", fill=fill)
        canvas.save(path)

    pair(DARK_BG, os.path.join(out_dir, "on-black.png"))
    pair((255, 255, 255), os.path.join(out_dir, "on-white.png"))

    bar = Image.new("RGBA", (560, 120), DARK_BG + (255,))
    draw = ImageDraw.Draw(bar)
    for i, (kind, label) in enumerate((("client", "PJDesk"), ("server", "PJDesk Server"))):
        icon = Image.fromarray(raster(svg_for(kind, 40), 40), "RGBA")
        x = 16 + i * 270
        bar.paste(icon, (x, 28), icon)
        draw.text((x + 48, 40), label, fill=(230, 232, 236, 255))
    bar.save(os.path.join(out_dir, "titlebar-dark.png"))

    # Native pixels, blown up with nearest neighbor so a torn edge would show.
    sheet_w = 16 * 10 + 32 * 8 + 48 * 6 + 80
    sheet = Image.new("RGBA", (sheet_w, 48 * 6 * 2 + 80), DARK_BG + (255,))
    light = Image.new("RGBA", sheet.size, (255, 255, 255, 255))
    draw = ImageDraw.Draw(sheet)
    draw2 = ImageDraw.Draw(light)
    x = 16
    for size, zoom in ((16, 10), (32, 8), (48, 6)):
        for row, kind in enumerate(("client", "server")):
            im = Image.fromarray(on_plate(images[kind][size], DARK_BG + (255,)), "RGBA")
            big = im.resize((size * zoom, size * zoom), Image.Resampling.NEAREST)
            y = 28 + row * (48 * 6 + 16)
            sheet.paste(big, (x, y))
            imw = Image.fromarray(on_plate(images[kind][size], (255, 255, 255, 255)), "RGBA")
            light.paste(imw.resize((size * zoom, size * zoom), Image.Resampling.NEAREST), (x, y))
        draw.text((x, 6), f"{size}px", fill=(200, 204, 210, 255))
        draw2.text((x, 6), f"{size}px", fill=(40, 44, 52, 255))
        x += size * zoom + 24
    sheet.save(os.path.join(out_dir, "samples-16-32-48-dark.png"))
    light.save(os.path.join(out_dir, "samples-16-32-48-light.png"))


def main():
    images = render_all()
    stats = verify(images)
    write_svgs()
    write_windows(images)
    write_android(images)
    write_site(images)
    write_previews(images)
    print(
        "client white", stats[0], "glass", stats[1], "blue", stats[2],
        "server white", stats[3], "blue", stats[4],
    )


if __name__ == "__main__":
    main()
