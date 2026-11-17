#!/usr/bin/env python3
"""Assemble owner-approved brand SVGs from Natural Earth coastlines + fixed track/plane geometry."""

from __future__ import annotations

import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COAST = ROOT / "assets" / "brand" / "coastlines.paths.json"

OCEAN_HI = "#4EB0FF"
OCEAN_LO = "#156BBF"
LAND = "#6DB33F"
GRID = "#0B3378"
BORDER = "#0D3A8C"
TRACK_COLOR = "#FF00CC"

S_PLANE = 1.12
T = (610.0, 425.0)
TAIL_SRC = (672.0, 450.0)
TIP_SRC = (982.0, 118.0)
AXIS = (TIP_SRC[0] - TAIL_SRC[0], TIP_SRC[1] - TAIL_SRC[1])
ANG = math.atan2(AXIS[1], AXIS[0])
U = (math.cos(ANG), math.sin(ANG))
P = (T[0] - 60 * U[0], T[1] - 60 * U[1])
C2 = (P[0] - 100 * U[0], P[1] - 100 * U[1])
T_END = (T[0] + 4 * U[0], T[1] + 4 * U[1])

TRACK_FULL = (
    "M 150,885 C 330,895 590,850 640,740 "
    "C 680,655 660,640 590,640 "
    "C 400,640 212,630 212,520 "
    f"C 205,195 {C2[0]:.0f},{C2[1]:.0f} {P[0]:.0f},{P[1]:.0f} "
    f"L {T_END[0]:.0f},{T_END[1]:.0f}"
)

TRACK_SMALL = TRACK_FULL

PLANE_XF = f"translate({T[0]} {T[1]}) scale({S_PLANE}) translate({-TAIL_SRC[0]} {-TAIL_SRC[1]})"

# Owner polygon from flightvis-logo-final.py / base.jpg, split into fill regions.
PLANE_WHITE_TOP = "M 672 450 L 650 328 L 536 280 L 982 118 L 745 398 Z"
PLANE_WHITE_LOWER = "M 672 450 L 745 398 L 815 446 L 650 328 Z"
PLANE_NAVY_KEEL = "M 672 450 L 815 446 L 745 398 Z"
PLANE_OUTLINE = "M 536 280 L 982 118 L 815 446 L 745 398 L 672 450 L 650 328 Z"


def load_map() -> dict:
    return json.loads(COAST.read_text(encoding="utf-8"))


def ortho_lonlat(lon: float, lat: float, lon0: float, lat0: float) -> tuple[float, float]:
    lam = math.radians(lon)
    phi = math.radians(lat)
    lam0 = math.radians(lon0)
    phi0 = math.radians(lat0)
    x = math.cos(phi) * math.sin(lam - lam0)
    y = math.cos(phi0) * math.sin(phi) - math.sin(phi0) * math.cos(phi) * math.cos(lam - lam0)
    return x, y


def graticule_lines(proj: dict, mx: float, my: float, mw: float, mh: float) -> str:
    lon0, lat0 = proj["lon0"], proj["lat0"]
    scale, ox, oy = proj["scale"], proj["ox"], proj["oy"]

    def scr(lon: float, lat: float) -> tuple[float, float]:
        gx, gy = ortho_lonlat(lon, lat, lon0, lat0)
        return ox + scale * gx, oy - scale * gy

    lines: list[str] = []
    for lon in range(-160, 41, 20):
        pts: list[str] = []
        for lat in range(-58, 82, 2):
            x, y = scr(lon, lat)
            if mx - 8 <= x <= mx + mw + 8 and my - 8 <= y <= my + mh + 8:
                pts.append(f"{x:.1f},{y:.1f}")
        if len(pts) >= 2:
            lines.append(f"M {pts[0]} L {' L '.join(pts[1:])}")
    for lat in range(-60, 61, 20):
        pts = []
        for lon in range(-175, 43, 2):
            x, y = scr(lon, lat)
            if mx - 8 <= x <= mx + mw + 8 and my - 8 <= y <= my + mh + 8:
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
    <g stroke="{GRID}" stroke-opacity="0.35" stroke-width="3" fill="none">
      {graticule}
    </g>"""

    inner_scale = ""
    outer_close = ""
    if maskable_pad != 1.0:
        inner_scale = (
            f'<g transform="translate(512 512) scale({maskable_pad}) translate(-512 -512)">'
        )
        outer_close = "</g>"

    plane_stroke = 10
    halo_stroke = 16
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024" role="img" aria-label="flight_vis">
  <defs>
    <clipPath id="mapClip">
      <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" rx="{rx}" ry="{rx}"/>
    </clipPath>
    <radialGradient id="oceanGrad" cx="46%" cy="42%" r="68%" fx="40%" fy="36%">
      <stop offset="0%" stop-color="{OCEAN_HI}"/>
      <stop offset="100%" stop-color="{OCEAN_LO}"/>
    </radialGradient>
  </defs>
  {inner_scale}
  <g clip-path="url(#mapClip)">
    <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" fill="url(#oceanGrad)"/>
    {grid_block}
    {coast_html}
  </g>
  <rect x="{mx}" y="{my}" width="{mw}" height="{mh}" rx="{rx}" ry="{rx}" fill="none" stroke="{BORDER}" stroke-width="14"/>
  <path fill="none" stroke="{TRACK_COLOR}" stroke-width="{track_w}" stroke-linecap="butt" stroke-dasharray="{track_dash}" d="{track_d}"/>
  <g transform="{PLANE_XF}">
    <path fill="#FFFFFF" stroke="none" d="{PLANE_WHITE_TOP}"/>
    <path fill="#FFFFFF" stroke="none" d="{PLANE_WHITE_LOWER}"/>
    <path fill="{BORDER}" stroke="none" d="{PLANE_NAVY_KEEL}"/>
    <path fill="none" stroke="#FFFFFF" stroke-width="{halo_stroke}" stroke-linejoin="round" d="{PLANE_OUTLINE}"/>
    <path fill="none" stroke="{BORDER}" stroke-width="{plane_stroke}" stroke-linejoin="round" d="{PLANE_OUTLINE}"/>
  </g>
  {outer_close}
</svg>
"""


def main() -> None:
    data = load_map()
    map_meta = data["map"]
    proj = data["projection"]
    graticule = graticule_lines(proj, map_meta["x"], map_meta["y"], map_meta["w"], map_meta["h"])
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