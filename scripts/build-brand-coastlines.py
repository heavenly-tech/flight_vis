#!/usr/bin/env python3
"""Project Natural Earth land (equal-scale orthographic) into logo map space."""

from __future__ import annotations

import json
import math
import subprocess
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image
from skimage import measure

ROOT = Path(__file__).resolve().parents[1]
DATA_DIR = ROOT / "scripts" / "data"
FIT_PATH = ROOT / "assets/brand/projection.fit.json"
NE_110 = DATA_DIR / "ne_110m_land.geojson"
NE_URL_110 = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_110m_land.geojson"
)
OUT_PATH = ROOT / "assets" / "brand" / "coastlines.paths.json"
BRAND_SOURCE = ROOT / "branding" / "source"
REF_PNG = BRAND_SOURCE / "flightvis-logo-final.png"

MAP_X, MAP_Y, MAP_W, MAP_H, MAP_RX = 72, 88, 880, 848, 56


def download_land() -> Path:
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    path = NE_110
    if not path.exists() or path.stat().st_size < 10_000:
        print(f"fetch {NE_URL_110}")
        urllib.request.urlretrieve(NE_URL_110, path)
    return path


def load_projection() -> dict:
    if not FIT_PATH.exists() or (REF_PNG.exists() and FIT_PATH.stat().st_mtime < REF_PNG.stat().st_mtime):
        subprocess.run(
            [str(ROOT / "scripts/fit-brand-map-projection.py")],
            cwd=ROOT,
            check=True,
        )
    return json.loads(FIT_PATH.read_text(encoding="utf-8"))


def ortho_xy(lon: float, lat: float, lon0_deg: float, lat0_deg: float) -> tuple[float, float]:
    lam = math.radians(lon)
    phi = math.radians(lat)
    lon0 = math.radians(lon0_deg)
    lat0 = math.radians(lat0_deg)
    x = math.cos(phi) * math.sin(lam - lon0)
    y = math.cos(lat0) * math.sin(phi) - math.sin(lat0) * math.cos(phi) * math.cos(lam - lon0)
    return x, y


def project(lon: float, lat: float, proj: dict) -> tuple[float, float]:
    gx, gy = ortho_xy(lon, lat, proj["lon0"], proj["lat0"])
    s, ox, oy = proj["scale"], proj["ox"], proj["oy"]
    return ox + s * gx, oy - s * gy


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
    return -175.0 <= lon <= 45.0 and -58.0 <= lat <= 85.0


def fmt(n: float) -> str:
    s = f"{n:.1f}".rstrip("0").rstrip(".")
    return s if s != "-0" else "0"


def reference_land_mask_crop() -> np.ndarray:
    ref = np.array(Image.open(REF_PNG).convert("RGB"))
    r, g, b = ref[..., 0], ref[..., 1], ref[..., 2]
    magenta = (r > 150) & (b > 140) & (g < 130)
    white = (r > 205) & (g > 205) & (b > 205)
    land = (g > 150) & (g > r + 25) & (g > b + 45) & (r < 145) & ~magenta & ~white
    return land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]


def raster_paths_mask(paths: list[str]) -> np.ndarray:
    from PIL import ImageDraw

    img = Image.new("L", (1024, 1024), 0)
    draw = ImageDraw.Draw(img)
    for d in paths:
        if not d:
            continue
        pts = []
        tokens = d.replace("M ", "").replace("Z", "").split(" L ")
        for tok in tokens:
            x, y = tok.split(",")
            pts.append((float(x), float(y)))
        if len(pts) >= 3:
            draw.polygon(pts, fill=255)
    crop = img.crop((MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H))
    return np.array(crop) > 127


def mask_iou(a: np.ndarray, b: np.ndarray) -> float:
    u = np.logical_or(a, b).sum()
    return float(np.logical_and(a, b).sum()) / float(u) if u else 0.0


def trace_chart_paths() -> tuple[list[str], float]:
    rgb = np.array(Image.open(REF_PNG).convert("RGB"))
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    magenta = (r > 150) & (b > 140) & (g < 130)
    white = (r > 205) & (g > 205) & (b > 205)
    mask = (
        (g > 150) & (g > r + 25) & (g > b + 45) & (r < 145) & ~magenta & ~white
    ).astype(np.uint8)
    ref_crop = reference_land_mask_crop()
    candidates: list[tuple[float, float, list[str]]] = []
    for eps in (1.0, 1.2, 1.4, 1.6, 1.8, 2.0, 2.3, 2.6, 3.0):
        paths: list[str] = []
        for cont in measure.find_contours(mask, 0.5):
            pts = [(float(col), float(row)) for row, col in cont]
            if ring_area(pts) < 900.0:
                continue
            simp = rdp(pts, eps)
            d = path_d(simp)
            if d:
                paths.append(d)
        paths.sort(key=len, reverse=True)
        score = mask_iou(raster_paths_mask(paths), ref_crop)
        if score >= 0.85:
            candidates.append((score, eps, paths))
    if candidates:
        candidates.sort(key=lambda t: (-t[0], t[1]))
        for score, _eps, paths in candidates:
            if sum(len(p) for p in paths) < 42_000:
                return paths, score
        return candidates[0][2], candidates[0][0]
    # fallback: best overall
    best = (0.0, 1.0, [])
    for eps in (1.0, 1.4, 1.8, 2.2, 2.8):
        paths = []
        for cont in measure.find_contours(mask, 0.5):
            pts = [(float(col), float(row)) for row, col in cont]
            if ring_area(pts) < 900.0:
                continue
            simp = rdp(pts, eps)
            d = path_d(simp)
            if d:
                paths.append(d)
        paths.sort(key=len, reverse=True)
        score = mask_iou(raster_paths_mask(paths), ref_crop)
        if score > best[0]:
            best = (score, eps, paths)
    return best[2], best[0]


def paths_from_natural_earth(gj: dict, proj: dict) -> list[str]:
    eps = 1.35
    min_area = 120.0
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
                projected = [project(lo, la, proj) for lo, la in ring]
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
    return paths


def path_d(points: list[tuple[float, float]]) -> str:
    if not points:
        return ""
    parts = [f"M {fmt(points[0][0])},{fmt(points[0][1])}"]
    for x, y in points[1:]:
        parts.append(f"L {fmt(x)},{fmt(y)}")
    parts.append("Z")
    return " ".join(parts)


def main() -> None:
    ne_path = download_land()
    proj = load_projection()
    with ne_path.open(encoding="utf-8") as fh:
        gj = json.load(fh)

    ne_paths = paths_from_natural_earth(gj, proj)
    ne_iou = mask_iou(raster_paths_mask(ne_paths), reference_land_mask_crop())
    paths, trace_iou = trace_chart_paths()
    coast_source = "chart-trace"
    land_iou = trace_iou
    if ne_iou >= 0.85:
        paths, coast_source, land_iou = ne_paths, "natural-earth-110m", ne_iou

    payload = {
        "map": {"x": MAP_X, "y": MAP_Y, "w": MAP_W, "h": MAP_H, "rx": MAP_RX},
        "projection": {
            "kind": "orthographic",
            "lon0": proj["lon0"],
            "lat0": proj["lat0"],
            "scale": proj["scale"],
            "ox": proj["ox"],
            "oy": proj["oy"],
            "iou_ne_full": ne_iou,
            "iou_full": land_iou,
            "coast_source": coast_source,
        },
        "paths": paths,
    }
    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    OUT_PATH.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    total_chars = sum(len(p) for p in paths)
    print(
        f"wrote {OUT_PATH} ({len(paths)} paths, {total_chars} chars, "
        f"land_IoU={land_iou:.4f}, NE_IoU={ne_iou:.4f}, source={coast_source})"
    )


if __name__ == "__main__":
    main()