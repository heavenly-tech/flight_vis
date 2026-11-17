#!/usr/bin/env python3
"""Assemble brand SVGs from chart.jpg vector extraction + fixed track/plane geometry."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHART_VEC = ROOT / "assets/brand/chart-vector.json"
EXTRACT = ROOT / "branding/source/extract-chart-vector.py"

BORDER = "#0D3A8C"
TRACK_COLOR = "#FF00CC"

S_PLANE = 1.12
T = (610.0, 425.0)
TAIL_SRC = (672.0, 450.0)
TIP_SRC = (982.0, 118.0)
AXIS = (TIP_SRC[0] - TAIL_SRC[0], TIP_SRC[1] - TAIL_SRC[1])
import math

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

PLANE_WHITE_TOP = "M 672 450 L 650 328 L 536 280 L 982 118 L 745 398 Z"
PLANE_WHITE_LOWER = "M 536 280 L 672 450 L 815 446 L 982 118 L 745 398 Z"
PLANE_WHITE_UNDER = "M 538 295 L 978 125 L 828 458 L 662 462 Z"
PLANE_NAVY_KEEL = "M 672 450 L 815 446 L 745 398 Z"
PLANE_OUTLINE = "M 536 280 L 982 118 L 815 446 L 745 398 L 672 450 L 650 328 Z"
PLANE_WHITE_BODY = PLANE_OUTLINE


def ensure_chart_vector() -> dict:
    chart_jpg = ROOT / "branding/source/chart.jpg"
    if not CHART_VEC.exists() or (
        chart_jpg.exists() and CHART_VEC.stat().st_mtime < chart_jpg.stat().st_mtime
    ):
        subprocess.run([sys.executable, str(EXTRACT)], cwd=ROOT, check=True)
    return json.loads(CHART_VEC.read_text(encoding="utf-8"))


def land_paths(paths: list[str], color: str) -> str:
    return "\n    ".join(
        f'<path fill="{color}" fill-rule="nonzero" d="{p}"/>' for p in paths
    )


def grid_paths(paths: list[str], color: str) -> str:
    return "\n      ".join(
        f'<path fill="none" stroke="{color}" stroke-width="2.5" stroke-opacity="0.35" d="{p}"/>'
        for p in paths
    )


def svg_doc(
    *,
    include_grid: bool,
    track_d: str,
    track_w: int,
    track_dash: str,
    land: list[str],
    graticule: list[str],
    map_meta: dict,
    colors: dict,
    maskable_pad: float = 1.0,
) -> str:
    mx, my, mw, mh, rx = (
        map_meta["x"],
        map_meta["y"],
        map_meta["w"],
        map_meta["h"],
        map_meta["rx"],
    )
    ocean_hi = colors["ocean_hi"]
    ocean_lo = colors["ocean_lo"]
    land_c = colors["land"]
    grid_c = colors["grid"]
    coast_html = land_paths(land, land_c)
    grid_block = ""
    if include_grid and graticule:
        grid_block = f"""
    <g>
      {grid_paths(graticule, grid_c)}
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
    <radialGradient id="oceanGrad" cx="48%" cy="44%" r="62%" fx="44%" fy="40%">
      <stop offset="0%" stop-color="{ocean_hi}"/>
      <stop offset="100%" stop-color="{ocean_lo}"/>
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
    <path fill="#FFFFFF" stroke="none" d="{PLANE_WHITE_BODY}"/>
    <path fill="#FFFFFF" stroke="none" d="{PLANE_WHITE_UNDER}"/>
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
    data = ensure_chart_vector()
    map_meta = data["map"]
    colors = data["colors"]
    metrics = data.get("metrics", {})
    out = ROOT / "assets/brand"
    grids = data.get("graticule_paths") or []

    full = svg_doc(
        include_grid=True,
        track_d=TRACK_FULL,
        track_w=26,
        track_dash="46 24",
        land=data["land_paths"],
        graticule=grids,
        map_meta=map_meta,
        colors=colors,
    )
    small = svg_doc(
        include_grid=False,
        track_d=TRACK_SMALL,
        track_w=34,
        track_dash="140 80",
        land=data.get("land_paths_small") or data["land_paths"][:10],
        graticule=[],
        map_meta=map_meta,
        colors=colors,
    )
    mask = svg_doc(
        include_grid=False,
        track_d=TRACK_FULL,
        track_w=22,
        track_dash="46 24",
        land=data.get("land_paths_small") or data["land_paths"][:12],
        graticule=[],
        map_meta=map_meta,
        colors=colors,
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
    print(
        f"land IoU vs ref-land-mask: {metrics.get('land_iou_ref_mask')}, "
        f"paths: {metrics.get('land_eps_full')} eps, {metrics.get('graticule_paths')} grid paths"
    )


if __name__ == "__main__":
    main()