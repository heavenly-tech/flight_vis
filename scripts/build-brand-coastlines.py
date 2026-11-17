#!/usr/bin/env python3
"""Project Natural Earth 110m land (orthographic, Americas) into logo map space."""

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

MAP_X, MAP_Y, MAP_W, MAP_H, MAP_RX = 72, 88, 880, 848, 56
LON0_DEG, LAT0_DEG = -75.0, 10.0
LON0, LAT0 = math.radians(LON0_DEG), math.radians(LAT0_DEG)

# lon, lat -> canvas x,y (chart.jpg / owner anchors; weight via repeats)
ANCHORS: list[tuple[tuple[float, float], tuple[float, float]]] = [
    ((-168.0, 64.0), (118.0, 248.0)),
    ((-168.0, 64.0), (118.0, 248.0)),
    ((-178.0, 52.0), (120.0, 260.0)),
    ((-66.0, -54.0), (510.0, 918.0)),
    ((-66.0, -54.0), (510.0, 918.0)),
    ((-66.0, -54.0), (510.0, 918.0)),
    ((-35.0, -5.0), (665.0, 670.0)),
    ((-5.0, 50.0), (872.0, 228.0)),
    ((-5.0, 50.0), (872.0, 228.0)),
    ((-5.0, 50.0), (872.0, 228.0)),
    ((5.0, 36.0), (912.0, 348.0)),
    ((10.0, -15.0), (890.0, 720.0)),
    ((-45.0, 72.0), (720.0, 128.0)),
]


def download_land() -> None:
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    if not NE_PATH.exists() or NE_PATH.stat().st_size < 10_000:
        print(f"fetch {NE_URL}")
        urllib.request.urlretrieve(NE_URL, NE_PATH)


def ortho_xy(lon: float, lat: float) -> tuple[float, float]:
    lam = math.radians(lon)
    phi = math.radians(lat)
    x = math.cos(phi) * math.sin(lam - LON0)
    y = math.cos(LAT0) * math.sin(phi) - math.sin(LAT0) * math.cos(phi) * math.cos(lam - LON0)
    return x, y


def lstsq_3(rows: list[tuple[float, float, float]], targets: list[float]) -> tuple[float, float, float]:
    ata = [[0.0] * 3 for _ in range(3)]
    atb = [0.0, 0.0, 0.0]
    for (a, b, c), t in zip(rows, targets):
        v = (a, b, c)
        for i in range(3):
            atb[i] += v[i] * t
            for j in range(3):
                ata[i][j] += v[i] * v[j]
    # Gaussian elimination
    n = 3
    aug = [row[:] + [atb[i]] for i, row in enumerate(ata)]
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
    return aug[0][3], aug[1][3], aug[2][3]


def fit_affine(
    anchors: list[tuple[tuple[float, float], tuple[float, float]]],
) -> tuple[float, float, float, float, float, float]:
    rows: list[tuple[float, float, float]] = []
    xs: list[float] = []
    ys: list[float] = []
    for (lon, lat), (tx, ty) in anchors:
        gx, gy = ortho_xy(lon, lat)
        rows.append((gx, gy, 1.0))
        xs.append(tx)
        ys.append(ty)
    a, b, c = lstsq_3(rows, xs)
    d, e, f = lstsq_3(rows, ys)
    return a, b, c, d, e, f


def project(
    lon: float,
    lat: float,
    aff: tuple[float, float, float, float, float, float],
) -> tuple[float, float]:
    gx, gy = ortho_xy(lon, lat)
    a, b, c, d, e, f = aff
    return a * gx + b * gy + c, d * gx + e * gy + f


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
    return -175.0 <= lon <= 42.0 and -58.0 <= lat <= 85.0


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

    aff = fit_affine(ANCHORS)
    eps = 1.9
    min_area = 180.0
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
                if not in_view(ring[0][0], ring[0][1]):
                    continue
                projected = [project(lo, la, aff) for lo, la in ring]
                if abs(ring_area(projected)) < min_area:
                    continue
                simp = rdp(projected, eps)
                if not any(
                    MAP_X - 40 <= x <= MAP_X + MAP_W + 40 and MAP_Y - 40 <= y <= MAP_Y + MAP_H + 40
                    for x, y in simp
                ):
                    continue
                dpath = path_d(simp)
                if dpath:
                    paths.append(dpath)

    paths.sort(key=len, reverse=True)
    payload = {
        "map": {"x": MAP_X, "y": MAP_Y, "w": MAP_W, "h": MAP_H, "rx": MAP_RX},
        "projection": {
            "kind": "orthographic",
            "lon0": LON0_DEG,
            "lat0": LAT0_DEG,
            "aff": {"a": aff[0], "b": aff[1], "c": aff[2], "d": aff[3], "e": aff[4], "f": aff[5]},
        },
        "paths": paths,
    }
    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    total_chars = sum(len(p) for p in paths)
    print(f"wrote {OUT_PATH} ({len(paths)} paths, {total_chars} chars)")
    for label, lon, lat, target in (
        ("Alaska", -168, 64, (118, 248)),
        ("Patagonia", -66, -54, (510, 918)),
        ("Brazil NE", -35, -5, (665, 670)),
        ("Europe", -5, 50, (872, 228)),
    ):
        bx, by = project(lon, lat, aff)
        print(f"{label} -> ({bx:.1f},{by:.1f}) target {target}")


if __name__ == "__main__":
    main()