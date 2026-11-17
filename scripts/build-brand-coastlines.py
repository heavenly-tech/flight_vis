#!/usr/bin/env python3
"""Project Natural Earth 110m land into logo map space and emit simplified SVG paths."""

from __future__ import annotations

import json
import math
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA_DIR = ROOT / "scripts" / "data"
NE_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_110m_land.geojson"
)
NE_PATH = DATA_DIR / "ne_110m_land.geojson"
OUT_PATH = ROOT / "assets" / "brand" / "coastlines.paths.json"

# Rounded map frame (1024 canvas), matches chart.jpg placement.
MAP_X, MAP_Y, MAP_W, MAP_H, MAP_RX = 72, 88, 880, 848, 56

# lon, lat -> x, y calibration (owner reference / Brazil NE ≈ 665,670).
ANCHORS: list[tuple[tuple[float, float], tuple[float, float]]] = [
    ((-35.0, -5.0), (665.0, 670.0)),
    ((-35.0, -5.0), (665.0, 670.0)),
    ((-35.0, -5.0), (665.0, 670.0)),
    ((-66.0, -54.5), (505.0, 905.0)),
    ((-168.0, 66.0), (158.0, 168.0)),
    ((-20.0, 63.5), (842.0, 188.0)),
    ((-118.0, 34.0), (410.0, 248.0)),
]


def download_land() -> None:
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    if NE_PATH.stat().st_size > 10_000:
        return
    print(f"fetch {NE_URL}")
    urllib.request.urlretrieve(NE_URL, NE_PATH)


def fit_affine(
    anchors: list[tuple[tuple[float, float], tuple[float, float]]],
) -> tuple[float, float, float, float, float, float]:
    """x = a*lon + b*lat + c, y = d*lon + e*lat + f (least squares)."""
    ata = [[0.0] * 3 for _ in range(3)]
    atx = [0.0, 0.0, 0.0]
    aty = [0.0, 0.0, 0.0]
    for (lon, lat), (x, y) in anchors:
        v = [lon, lat, 1.0]
        for i in range(3):
            for j in range(3):
                ata[i][j] += v[i] * v[j]
            atx[i] += v[i] * x
            aty[i] += v[i] * y

    def solve(m: list[list[float]], b: list[float]) -> list[float]:
        n = 3
        aug = [row[:] + [b[i]] for i, row in enumerate(m)]
        for col in range(n):
            pivot = max(range(col, n), key=lambda r: abs(aug[r][col]))
            aug[col], aug[pivot] = aug[pivot], aug[col]
            div = aug[col][col]
            if abs(div) < 1e-12:
                continue
            aug[col] = [v / div for v in aug[col]]
            for row in range(n):
                if row == col:
                    continue
                factor = aug[row][col]
                aug[row] = [aug[row][j] - factor * aug[col][j] for j in range(n + 1)]
        return [aug[i][n] for i in range(n)]

    axb = solve(ata, atx)
    ayb = solve(ata, aty)
    return axb[0], axb[1], axb[2], ayb[0], ayb[1], ayb[2]


def project(
    lon: float, lat: float, a: float, b: float, c: float, d: float, e: float, f: float
) -> tuple[float, float]:
    return a * lon + b * lat + c, d * lon + e * lat + f


def perp_dist(point: tuple[float, float], a: tuple[float, float], b: tuple[float, float]) -> float:
    px, py = point
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = max(0, min(1, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    qx, qy = ax + t * dx, ay + t * dy
    return math.hypot(px - qx, py - qy)


def rdp(points: list[tuple[float, float]], eps: float) -> list[tuple[float, float]]:
    if len(points) < 2:
        return points
    lo, hi = points[0], points[-1]
    dist, idx = 0.0, 0
    for i in range(1, len(points) - 1):
        d = perp_dist(points[i], lo, hi)
        if d > dist:
            dist, idx = d, i
    if dist > eps:
        left = rdp(points[: idx + 1], eps)
        right = rdp(points[idx:], eps)
        return left[:-1] + right
    return [lo, hi]


def ring_area(coords: list[tuple[float, float]]) -> float:
    area = 0.0
    for i in range(len(coords)):
        x1, y1 = coords[i]
        x2, y2 = coords[(i + 1) % len(coords)]
        area += x1 * y2 - x2 * y1
    return area * 0.5


def in_view(lon: float, lat: float) -> bool:
    return -175.0 <= lon <= 30.0 and -58.0 <= lat <= 84.0


def fmt(n: float) -> str:
    s = f"{n:.1f}".rstrip("0").rstrip(".")
    return s if s != "-0" else "0"


def path_d(points: list[tuple[float, float]]) -> str:
    if not points:
        return ""
    parts = [f"M {fmt(points[0][0])},{fmt(points[0][1])}"]
    for x, y in points[1:]:
        parts.append(f"L {fmt(x)},{fmt(y)}")
    parts.append("Z")
    return " ".join(parts)


def main() -> None:
    download_land()
    with NE_PATH.open(encoding="utf-8") as fh:
        gj = json.load(fh)

    a, b, c, d, e, f = fit_affine(ANCHORS)
    eps = 2.4
    min_area = 220.0
    paths: list[str] = []

    for feat in gj.get("features", []):
        geom = feat.get("geometry") or {}
        gtype = geom.get("type")
        polys: list[list[list[float]]] = []
        if gtype == "Polygon":
            polys.append(geom["coordinates"])
        elif gtype == "MultiPolygon":
            polys.extend(geom["coordinates"])
        else:
            continue

        for poly in polys:
            for ring in poly:
                if len(ring) < 4:
                    continue
                sample = ring[0]
                if not in_view(sample[0], sample[1]):
                    continue
                projected = [project(lo, la, a, b, c, d, e, f) for lo, la in ring]
                if ring_area(projected) < min_area:
                    continue
                simp = rdp(projected, eps)
                x0, x1 = MAP_X, MAP_X + MAP_W
                y0, y1 = MAP_Y, MAP_Y + MAP_H
                if not any(x0 - 30 <= x <= x1 + 30 and y0 - 30 <= y <= y1 + 30 for x, y in simp):
                    continue
                dpath = path_d(simp)
                if dpath:
                    paths.append(dpath)

    paths.sort(key=len, reverse=True)
    payload = {
        "map": {"x": MAP_X, "y": MAP_Y, "w": MAP_W, "h": MAP_H, "rx": MAP_RX},
        "affine": {"a": a, "b": b, "c": c, "d": d, "e": e, "f": f},
        "paths": paths,
    }
    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    total_chars = sum(len(p) for p in paths)
    print(f"wrote {OUT_PATH} ({len(paths)} paths, {total_chars} chars)")


if __name__ == "__main__":
    main()