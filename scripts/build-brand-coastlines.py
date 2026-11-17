#!/usr/bin/env python3
"""Project Natural Earth land (equal-scale orthographic) into logo map space."""

from __future__ import annotations

import json
import math
import subprocess
import urllib.request
from pathlib import Path

import sys

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
from geo_clip import dot_visible, prepare_rings  # noqa: E402
from skimage import measure  # noqa: E402

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
CHART_JPG = BRAND_SOURCE / "chart.jpg"

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


def in_bbox(lon: float, lat: float) -> bool:
    return -175.0 <= lon <= 50.0 and -55.0 <= lat <= 85.0


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


def _reject_pacific_ghost(pts: list[tuple[float, float]]) -> bool:
    if len(pts) < 4:
        return True
    mid_ocean = sum(1 for x, y in pts if x < 295 and 280 < y < 780) / len(pts)
    if mid_ocean > 0.55:
        return True
    xs = [x for x, _ in pts]
    if max(xs) - min(xs) > MAP_W * 0.95:
        return True
    return False


def paths_from_chart_jpg() -> tuple[list[str], float]:
    """Illustrated coastlines from owner chart (labeled components, canvas space)."""
    from scipy.ndimage import label

    rgb = np.array(Image.open(CHART_JPG).convert("RGB"))
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    mask = ((g > 150) & (g > r + 25) & (g > b + 45) & (r < 145)).astype(np.uint8)
    crop = mask[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    ref_crop = reference_land_mask_crop()
    lbl, _ = label(crop)
    best = (0.0, [])
    for overlap in (0.5, 0.55, 0.6):
        for eps in (0.75, 0.85, 0.95, 1.05):
            paths: list[str] = []
            for k in range(1, int(lbl.max()) + 1):
                comp = lbl == k
                if comp.sum() < 500:
                    continue
                if (comp & ref_crop).sum() / comp.sum() < overlap:
                    continue
                for cont in measure.find_contours(comp.astype(np.uint8), 0.5):
                    pts = [(float(col) + MAP_X, float(row) + MAP_Y) for row, col in cont]
                    if abs(ring_area(pts)) < 900:
                        continue
                    simp = subdivide_screen_edges(rdp(pts, eps), max_len=100)
                    clipped = clip_polygon_rect(
                        simp, MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H
                    )
                    if len(clipped) >= 3 and not _reject_pacific_ghost(clipped):
                        d = path_d(clipped)
                        if d:
                            paths.append(d)
            score = mask_iou(raster_paths_mask(paths), ref_crop)
            if score > best[0]:
                best = (score, paths)
    return best[1], best[0]


def paths_from_ne_raster(gj: dict, proj: dict) -> list[str]:
    """Rasterize visible hemisphere land, then trace contours (avoids bow-tie polygons)."""
    from PIL import ImageDraw

    lon0, lat0 = proj["lon0"], proj["lat0"]
    scale, ox, oy = proj["scale"], proj["ox"], proj["oy"]
    w, h = MAP_W, MAP_H
    img = Image.new("L", (w, h), 0)
    draw = ImageDraw.Draw(img)

    for feat in gj.get("features", []):
        geom = feat.get("geometry") or {}
        polys: list = []
        if geom.get("type") == "Polygon":
            polys.append(geom["coordinates"])
        elif geom.get("type") == "MultiPolygon":
            polys.extend(geom["coordinates"])
        else:
            continue
        for poly in polys:
            for ring in poly:
                if len(ring) < 4:
                    continue
                for clipped in prepare_rings(
                    [(float(lo), float(la)) for lo, la in ring],
                    lon0,
                    lat0,
                    drop_antarctica_below=-58.0,
                ):
                    pts: list[tuple[float, float]] = []
                    for lo, la in clipped:
                        if dot_visible(lo, la, lon0, lat0) <= 0:
                            continue
                        gx, gy = ortho_xy(lo, la, lon0, lat0)
                        pts.append((ox + scale * gx - MAP_X, oy - scale * gy - MAP_Y))
                    if len(pts) >= 3:
                        xs = [p[0] for p in pts]
                        if max(xs) - min(xs) > MAP_W * 0.55:
                            continue
                        if abs(ring_area(pts)) > 90_000:
                            continue
                        draw.polygon(pts, fill=255)

    land = np.array(img) > 127
    paths: list[str] = []
    for eps in (0.85, 1.0, 1.15):
        trial: list[str] = []
        for cont in measure.find_contours(land.astype(np.uint8), 0.5):
            pts = [(float(col) + MAP_X, float(row) + MAP_Y) for row, col in cont]
            if abs(ring_area(pts)) < 800:
                continue
            simp = subdivide_screen_edges(rdp(pts, eps), max_len=90)
            clipped = clip_polygon_rect(simp, MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H)
            if len(clipped) >= 3:
                d = path_d(clipped)
                if d:
                    trial.append(d)
        score = mask_iou(raster_paths_mask(trial), reference_land_mask_crop())
        if score >= 0.85 or not paths:
            paths, best_eps = trial, eps
        if score >= 0.86:
            break
    paths.sort(key=len, reverse=True)
    return paths


def paths_from_reference_mask() -> list[str]:
    ref = reference_land_mask_crop().astype(np.uint8)
    paths: list[str] = []
    for cont in measure.find_contours(ref, 0.5):
        pts = [(float(col) + MAP_X, float(row) + MAP_Y) for row, col in cont]
        if abs(ring_area(pts)) < 400:
            continue
        simp = subdivide_screen_edges(rdp(pts, 0.85), max_len=100)
        clipped = clip_polygon_rect(simp, MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H)
        if len(clipped) >= 3:
            d = path_d(clipped)
            if d:
                paths.append(d)
    paths.sort(key=len, reverse=True)
    return paths


def paths_from_natural_earth(gj: dict, proj: dict) -> list[str]:
    eps = 1.35
    min_area = 120.0
    lon0, lat0 = proj["lon0"], proj["lat0"]
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
                if not in_bbox(ring[0][0], ring[0][1]):
                    continue
                raw = [(float(lo), float(la)) for lo, la in ring]
                for clipped in prepare_rings(raw, lon0, lat0):
                    projected = [project(lo, la, proj) for lo, la in clipped]
                    if abs(ring_area(projected)) < min_area:
                        continue
                    simp = subdivide_screen_edges(rdp(projected, eps))
                    clipped = clip_polygon_rect(
                        simp,
                        MAP_X,
                        MAP_Y,
                        MAP_X + MAP_W,
                        MAP_Y + MAP_H,
                    )
                    if len(clipped) < 3 or abs(ring_area(clipped)) < min_area:
                        continue
                    xs = [x for x, _ in clipped]
                    if max(xs) - min(xs) > MAP_W * 0.97:
                        continue
                    dpath = path_d(clipped)
                    if dpath:
                        paths.append(dpath)
    paths.sort(key=len, reverse=True)
    return paths


def _inside_left(p: tuple[float, float], x: float) -> bool:
    return p[0] >= x


def _inside_right(p: tuple[float, float], x: float) -> bool:
    return p[0] <= x


def _inside_bottom(p: tuple[float, float], y: float) -> bool:
    return p[1] >= y


def _inside_top(p: tuple[float, float], y: float) -> bool:
    return p[1] <= y


def _intersect(
    a: tuple[float, float], b: tuple[float, float], edge: str, v: float
) -> tuple[float, float]:
    ax, ay = a
    bx, by = b
    if edge in ("left", "right"):
        if abs(bx - ax) < 1e-12:
            return (v, ay)
        t = (v - ax) / (bx - ax)
        return (v, ay + t * (by - ay))
    if abs(by - ay) < 1e-12:
        return (ax, v)
    t = (v - ay) / (by - ay)
    return (ax + t * (bx - ax), v)


def subdivide_screen_edges(
    pts: list[tuple[float, float]], max_len: float = 180.0
) -> list[tuple[float, float]]:
    """Insert vertices along screen-space edges — never break a closed ring into open chains."""
    if len(pts) < 2:
        return pts
    closed = pts[0] == pts[-1] or (
        len(pts) > 2
        and abs(pts[0][0] - pts[-1][0]) < 1e-6
        and abs(pts[0][1] - pts[-1][1]) < 1e-6
    )
    chain = pts[:-1] if closed and len(pts) > 1 else pts[:]
    if len(chain) < 2:
        return pts
    out: list[tuple[float, float]] = [chain[0]]
    n = len(chain)
    for i in range(n):
        a = chain[i]
        b = chain[(i + 1) % n]
        dx, dy = b[0] - a[0], b[1] - a[1]
        dist = math.hypot(dx, dy)
        if dist > max_len:
            steps = int(math.ceil(dist / max_len))
            for s in range(1, steps):
                t = s / steps
                out.append((a[0] + t * dx, a[1] + t * dy))
        if i < n - 1 or closed:
            if not out or out[-1] != b:
                out.append(b)
    if closed and out and out[0] != out[-1]:
        out.append(out[0])
    return out


def clip_polygon_rect(
    poly: list[tuple[float, float]],
    xmin: float,
    ymin: float,
    xmax: float,
    ymax: float,
) -> list[tuple[float, float]]:
    """Sutherland–Hodgman clip to map frame."""
    out = poly[:]
    for edge, val, inside, is_x in (
        ("left", xmin, _inside_left, True),
        ("right", xmax, _inside_right, True),
        ("bottom", ymin, _inside_bottom, False),
        ("top", ymax, _inside_top, False),
    ):
        if not out:
            break
        inp, out = out, []
        for i in range(len(inp)):
            cur = inp[i]
            prev = inp[i - 1]
            c_in = inside(cur, val)
            p_in = inside(prev, val)
            if c_in:
                if not p_in:
                    out.append(_intersect(prev, cur, edge, val))
                out.append(cur)
            elif p_in:
                out.append(_intersect(prev, cur, edge, val))
    return out


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

    ne_vector = paths_from_natural_earth(gj, proj)
    ne_iou = mask_iou(raster_paths_mask(ne_vector), reference_land_mask_crop())
    ne_raster = paths_from_ne_raster(gj, proj)
    ne_raster_iou = mask_iou(raster_paths_mask(ne_raster), reference_land_mask_crop())
    chart_paths, chart_iou = paths_from_chart_jpg()
    ref_paths = paths_from_reference_mask()
    ref_iou = mask_iou(raster_paths_mask(ref_paths), reference_land_mask_crop())

    candidates = [
        (ne_raster_iou, ne_raster, "natural-earth-110m-raster"),
        (ne_iou, ne_vector, "natural-earth-110m"),
        (chart_iou, chart_paths, "chart-jpg"),
        (ref_iou, ref_paths, "reference-mask"),
    ]
    candidates.sort(key=lambda t: t[0], reverse=True)
    land_iou, paths, coast_source = candidates[0]
    if land_iou < 0.85:
        land_iou, paths, coast_source = ref_iou, ref_paths, "reference-mask"

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
            "iou_ne_raster": ne_raster_iou,
            "iou_chart": chart_iou,
            "iou_reference_mask": ref_iou,
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