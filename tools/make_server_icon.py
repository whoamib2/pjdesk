#!/usr/bin/env python3
"""Generates a distinct PJDesk *Server* icon: a green server stack on a dark
rounded base (clearly different from the client's blue monitor)."""
import os
from PIL import Image, ImageDraw

S = 1024
A = os.path.join(os.path.dirname(__file__), "..", "crates", "pjdesk-server", "assets")
os.makedirs(A, exist_ok=True)

BASE_TOP = (26, 32, 44, 255)
BASE_BOT = (11, 14, 19, 255)
BORDER = (42, 49, 58, 255)
UNIT = (36, 44, 56, 255)
UNIT_HI = (48, 58, 72, 255)
GREEN = (63, 207, 107, 255)   # #3fcf6b — server/online accent
WHITE = (236, 242, 250, 255)


def vgrad(w, h, top, bot):
    g = Image.new("RGBA", (w, h))
    px = g.load()
    for y in range(h):
        t = y / max(1, h - 1)
        row = tuple(round(top[i] + (bot[i] - top[i]) * t) for i in range(4))
        for x in range(w):
            px[x, y] = row
    return g


img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# Dark rounded base (same family as the client icon).
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, S - 1, S - 1], radius=232, fill=255)
img.paste(vgrad(S, S, BASE_TOP, BASE_BOT), (0, 0), mask)
d.rounded_rectangle([6, 6, S - 7, S - 7], radius=228, outline=BORDER, width=8)

# Three stacked server units, each with a green status LED and vent slots.
x0, x1 = 236, S - 236
unit_h = 150
gap = 52
top = 250
for i in range(3):
    y0 = top + i * (unit_h + gap)
    y1 = y0 + unit_h
    d.rounded_rectangle([x0, y1 - 8, x1, y1 + 6], radius=28, fill=(20, 25, 33, 255))  # shadow lip
    d.rounded_rectangle([x0, y0, x1, y1], radius=30, fill=UNIT)
    d.rounded_rectangle([x0, y0, x1, y0 + unit_h // 2], radius=30, fill=UNIT_HI)
    d.rectangle([x0, y0 + unit_h // 3, x1, y1 - 20], fill=UNIT)
    # Status LED (glowing green).
    cx, cy = x0 + 84, y0 + unit_h // 2
    d.ellipse([cx - 40, cy - 40, cx + 40, cy + 40], fill=(23, 60, 40, 255))
    d.ellipse([cx - 22, cy - 22, cx + 22, cy + 22], fill=GREEN)
    # Vent slots.
    for v in range(4):
        vx = x1 - 260 + v * 56
        d.rounded_rectangle([vx, cy - 26, vx + 26, cy + 26], radius=12, fill=(22, 27, 35, 255))

# Downscale + export.
icon = img.resize((256, 256), Image.LANCZOS)
icon.save(os.path.join(A, "pjdesk-server.ico"),
          sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
icon.save(os.path.join(A, "icon_server_256.png"))
for n in (256, 32):
    open(os.path.join(A, f"icon_server_{n}.rgba"), "wb").write(
        img.resize((n, n), Image.LANCZOS).convert("RGBA").tobytes())
print("wrote", sorted(os.listdir(A)))
