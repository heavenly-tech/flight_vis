#!/usr/bin/env python3
"""Approved raster logo compose (chart + dashed track + masked plane)."""

from __future__ import annotations

import io
import math
from pathlib import Path

import cairosvg
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent

src = Image.open(HERE / "base.jpg").convert("RGB")
base = np.array(src).astype(int)
H, W, _ = base.shape
poly = [(536, 280), (982, 118), (815, 446), (745, 398), (672, 450), (650, 328)]
pm = Image.new("L", (W, H), 0)
ImageDraw.Draw(pm).polygon(poly, fill=255)
pm = np.array(pm) > 0
R, G, B = base[..., 0], base[..., 1], base[..., 2]
keep = pm & (
    ((R > 200) & (G > 200) & (B > 200)) | ((B > R + 20) & (G < 110) & (R < 80))
)
mask = (
    Image.fromarray((keep * 255).astype("uint8"))
    .filter(ImageFilter.MaxFilter(3))
    .filter(ImageFilter.GaussianBlur(0.8))
)
plane = src.convert("RGBA")
plane.putalpha(mask)
tail0 = np.array([672.0, 450.0])
tip0 = np.array([982.0, 118.0])
v = tip0 - tail0
old = math.atan2(v[1], v[0])
new = old
d = new - old
S = 1.12
T = np.array([610.0, 425.0])
A = S * np.array([[math.cos(d), -math.sin(d)], [math.sin(d), math.cos(d)]])
Ai = np.linalg.inv(A)
c = tail0 - Ai @ T
plane2 = plane.transform(
    (W, H),
    Image.AFFINE,
    (Ai[0, 0], Ai[0, 1], c[0], Ai[1, 0], Ai[1, 1], c[1]),
    resample=Image.BICUBIC,
)
u = np.array([math.cos(new), math.sin(new)])
P = T - 60 * u
c2 = P - 100 * u
d_ = (
    f"M150,885 C 330,895 590,850 640,740 C 680,655 660,640 590,640 "
    f"C 400,640 212,630 212,520 C 205,195 {c2[0]:.0f},{c2[1]:.0f} {P[0]:.0f},{P[1]:.0f} "
    f"L {T[0] + 4 * u[0]:.0f},{T[1] + 4 * u[1]:.0f}"
)
print(d_, "tip", A @ v + T)
chart = Image.open(HERE / "chart.jpg").convert("RGBA")
svg = (
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">'
    f'<path d="{d_}" fill="none" stroke="#FF00CC" stroke-width="26" '
    f'stroke-linecap="butt" stroke-dasharray="46 24"/></svg>'
)
ov = Image.open(io.BytesIO(cairosvg.svg2png(bytestring=svg.encode()))).convert("RGBA")
out = Image.alpha_composite(chart, ov)
out = Image.alpha_composite(out, plane2)
out.convert("RGB").save(HERE / "flightvis-logo-final.png")
print(f"wrote {HERE / 'flightvis-logo-final.png'}")