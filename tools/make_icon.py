#!/usr/bin/env python3
"""Generates the PJDesk app icon: a blue monitor with a remote cursor on a dark
rounded base. Outputs a multi-size .ico plus raw RGBA blobs for the window/tray."""
import os
from PIL import Image, ImageDraw

S = 1024  # supersample canvas, downscaled at the end
A = os.path.join(os.path.dirname(__file__), "..", "crates", "pjdesk", "assets")
os.makedirs(A, exist_ok=True)

# Palette (matches the app theme).
BASE_TOP = (26, 32, 44, 255)     # #1a202c
BASE_BOT = (11, 14, 19, 255)     # #0b0e13
BORDER = (42, 49, 58, 255)       # #2a313e
BLUE = (77, 155, 255, 255)       # #4d9bff accent
BLUE_DK = (43, 110, 214, 255)    # deeper blue for the screen base
WHITE = (240, 244, 250, 255)
INK = (10, 16, 28, 255)


def rounded(size, radius, fill):
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=fill)
    return img


def vgrad(w, h, top, bot):
    g = Image.new("RGBA", (w, h))
    px = g.load()
    for y in range(h):
        t = y / max(1, h - 1)
        px_row = tuple(round(top[i] + (bot[i] - top[i]) * t) for i in range(4))
        for x in range(w):
            px[x, y] = px_row
    return g


img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)

# Dark rounded base with a subtle vertical gradient + hairline border.
base_mask = rounded(S, 232, (255, 255, 255, 255)).split()[3]
grad = vgrad(S, S, BASE_TOP, BASE_BOT)
img.paste(grad, (0, 0), base_mask)
d.rounded_rectangle([6, 6, S - 7, S - 7], radius=228, outline=BORDER, width=8)

# Monitor screen (rounded rect) with a deeper-blue base for a touch of depth.
mx0, my0, mx1, my1 = 232, 236, S - 232, S - 320
d.rounded_rectangle([mx0, my0 + 10, mx1, my1 + 10], radius=70, fill=BLUE_DK)
d.rounded_rectangle([mx0, my0, mx1, my1], radius=70, fill=BLUE)
# A slim darker top bar, like a window.
d.rounded_rectangle([mx0, my0, mx1, my0 + 96], radius=70, fill=(32, 92, 196, 255))
d.rectangle([mx0, my0 + 60, mx1, my0 + 96], fill=(32, 92, 196, 255))
for i in range(3):  # window "dots"
    cx = mx0 + 70 + i * 60
    d.ellipse([cx - 16, my0 + 34, cx + 16, my0 + 66], fill=(210, 228, 255, 230))

# Monitor stand.
cx = S // 2
d.polygon([(cx - 70, my1), (cx + 70, my1), (cx + 44, my1 + 96), (cx - 44, my1 + 96)], fill=BORDER)
d.rounded_rectangle([cx - 150, my1 + 92, cx + 150, my1 + 126], radius=16, fill=BORDER)

# Remote cursor (classic pointer) on the screen, white with a dark outline.
ox, oy, k = cx - 70, my0 + 180, 15.5
pts = [(0, 0), (0, 17.5), (4.2, 13.2), (7.2, 20.0), (10.2, 18.6), (7.0, 12.0), (12.5, 12.0)]
poly = [(ox + x * k, oy + y * k) for (x, y) in pts]
d.polygon(poly, fill=WHITE, outline=INK)
d.line(poly + [poly[0]], fill=INK, width=6, joint="curve")

# Downscale and export.
icon = img.resize((256, 256), Image.LANCZOS)
icon.save(os.path.join(A, "pjdesk.ico"), sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
icon.save(os.path.join(A, "icon_256.png"))
for n in (256, 32):
    img.resize((n, n), Image.LANCZOS).convert("RGBA").tobytes()
    open(os.path.join(A, f"icon_{n}.rgba"), "wb").write(img.resize((n, n), Image.LANCZOS).convert("RGBA").tobytes())
print("wrote", os.listdir(A))
