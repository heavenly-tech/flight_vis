#!/usr/bin/env python3
"""Assemble owner-approved brand SVGs from Natural Earth coastlines + fixed track/plane geometry."""

from __future__ import annotations

import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COAST = ROOT / "assets" / "brand" / "coastlines.paths.json"

OCEAN = "#1E90FF"
LAND = "#6DB33F"
GRID = "#0D3A8C"
BORDER = "#0D3A8C"
TRACK_COLOR = "#FF00CC"

S_PLANE = 1.12
T = (610.0, 425.0)
TAIL_SRC = (672.0, 450.0)
TIP_SRC = (982.0, 118.0)
AXIS = (TIP_SRC[0] - TAIL_SRC[0], TIP_SRC[1] - TAIL_SRC[1])
U = (AXIS[0] / math.hypot(*AXIS), AXIS[1] / math.hypot(*AXIS))
P = (T[0] - 60 * U[0], T[1] - 60 * U[1])
C2 = (P[0] - 100 * U[0], P[1] - 100 * U[1])

TRACK_FULL = (
    "M 150,885 C 330,895 590,850 640,740 "
    "C 680,655 660,640 590,640 "
    "C 400,640 212,630 212,520 "
    f"C 205,195 {C2[0]:.2f},{C2[1]:.2f} {P[0]:.2f},{P[1]:.2f} "
    f"L {T[0]:.0f},{T[1]:.0f}"
)

TRACK_SMALL = (
    "M 150,885 C 330,895 590,850 640,740 "
    "C 680,655 660,640 590,640 "
    "C 400,640 212,630 212,520 "
    f"C 205,195 {C2[0]:.1f},{C2[1]:.1f} {P[0]:.1f},{P[1]:.1f} "
    f"L {T[0]:.0f},{T[1]:.0f}"
)

PLANE_XF = f"translate({T[0]} {T[1]}) scale({S_PLANE}) translate({-TAIL_SRC[0]} {-TAIL_SRC[1]})"

# Paper plane in base.jpg space: large white top, navy lower fold, thick outline.
PLANE_TOP = "M 672 450 L 702 278 L 982 118 L 812 215 L 672 450 Z"
PLANE_BOTTOM = "M 672 450 L 812 215 L 912 432 L 692 438 Z"
PLANE_KEEL = "M 672 450 L 905 188"


def load_map() -> dict:
    return json.loads(COAST.read_text(encoding="utf-8"))


def graticule_lines(affine: dict, mx: float, my: float, mw: float, mh: float) -> str:
    a, b, c, d, e, f = (
        affine["a"],
        affine["b"],
        affine["c"],
        affine["d"],
        affine["e"],
        affine["f"],
    )

    def proj(lon: float, lat: float) -> tuple[float, float]:
        return a * lon + b * lat + c, d * lon + e * lat + f

    lines: list[str] = []
    for lon in range(-150, 31, 30):
        pts: list[str] = []
        for lat in range(-60, 81, 5):
            x, y = proj(lon, lat)
            if mx - 5 <= x <= mx + mw + 5 and my - 5 <= y <= my + mh + 5:
                pts.append(f"{x:.1f},{y:.1f}")
        if len(pts) >= 2:
            lines.append(f"M {pts[0]} L {' L '.join(pts[1:])}")
    for lat in range(-60, 61, 30):
        pts = []
        for lon in range(-165, 26, 5):
            x, y = proj(lon, lat)
            if mx - 5 <= x <= mx + mw + 5 and my - 5 <= y <= my + mh + 5:
                pts.append(f"{x:.1f},{y:.1f}")
        if len(pts) >= 2:
            lines.append(f"M {pts[0]} L {' L '.join(pts[1:])}")
    return "\n      ".join(lines)


def coast_paths(map_data: dict, *, top_n: int | None = None) -> str:
    paths = map_data["paths"]
    if top_n:
        paths = sorted(paths, key=len, reverse=True)[:top_n]
    return "\n    ".join(f'<path fill="{LAND}" d="{p}"/>' for p in paths)


def svg_doc(
    *,
    include_grid: bool,
    track_d: str,
    track_w: int,
    track_dash: str,
    coast_html: str,
    graticule: str,
    map_meta: dict,
    maskable_pad: float = 1.0,
) -> str:
    mx, my, mw, mh, rx = (
        map_meta["x"],
        map_meta["y"],
        map_meta["w"],
        map_meta["h"],
        map_meta["rx"],
    )
    grid_block = ""
    if include_grid and graticule:
        grid_block = f"""
    <g clip-path="url(#mapClip)" stroke="{GRID}" stroke-opacity="0.12" stroke-width="2" fill="none">
      {graticule}
    </g>"""

    inner_scale = ""
    outer_close = ""
    if maskable_pad != 1.0:
        inner_scale = (
            f'<g transform="translate(512 512) scale({maskable_pad}) translate(-512 -512)">'
        )
        outer_close = "</g>"

    plane_stroke = 12 if maskable_pad != 1.0 else 11
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024" role="img" aria-label="flight_vis">
  <defs>
    <clipPath id="mapClip">
      <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" rx="{rx}" ry="{rx}"/>
    </clipPath>
  </defs>
  {inner_scale}
  <g clip-path="url(#mapClip)">
    <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" fill="{OCEAN}"/>
    {coast_html}
  </g>{grid_block}
  <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" rx="{rx}" ry="{rx}" fill="none" stroke="{BORDER}" stroke-width="14"/>
  <path fill="none" stroke="{TRACK_COLOR}" stroke-width="{track_w}" stroke-linecap="butt" stroke-dasharray="{track_dash}" d="{track_d}"/>
  <g transform="{PLANE_XF}">
    <path fill="#FFFFFF" stroke="none" d="{PLANE_TOP}"/>
    <path fill="{BORDER}" stroke="none" d="{PLANE_BOTTOM}"/>
    <path fill="none" stroke="{BORDER}" stroke-width="{plane_stroke + 2}" stroke-linejoin="round" d="{PLANE_TOP}"/>
    <path fill="none" stroke="{BORDER}" stroke-width="{plane_stroke + 2}" stroke-linejoin="round" d="{PLANE_BOTTOM}"/>
    <path fill="none" stroke="#FFFFFF" stroke-width="{max(plane_stroke - 4, 5)}" stroke-linecap="round" opacity="0.85" d="{PLANE_KEEL}"/>
  </g>
  {outer_close}
</svg>
"""


def main() -> None:
    data = load_map()
    map_meta = data["map"]
    graticule = graticule_lines(data["affine"], map_meta["x"], map_meta["y"], map_meta["w"], map_meta["h"])
    out = ROOT / "assets" / "brand"

    full = svg_doc(
        include_grid=True,
        track_d=TRACK_FULL,
        track_w=26,
        track_dash="46 24",
        coast_html=coast_paths(data),
        graticule=graticule,
        map_meta=map_meta,
    )
    small = svg_doc(
        include_grid=False,
        track_d=TRACK_SMALL,
        track_w=34,
        track_dash="140 80",
        coast_html=coast_paths(data, top_n=10),
        graticule="",
        map_meta=map_meta,
    )
    mask = svg_doc(
        include_grid=False,
        track_d=TRACK_FULL,
        track_w=22,
        track_dash="46 24",
        coast_html=coast_paths(data, top_n=12),
        graticule="",
        map_meta=map_meta,
        maskable_pad=0.82,
    )

    for name, body in (
        ("flightvis-logo.svg", full),
        ("flightvis-logo-small.svg", small),
        ("flightvis-logo-maskable.svg", mask),
    ):
        path = out / name
        path.write_text(body, encoding="utf-8")
        print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    main()