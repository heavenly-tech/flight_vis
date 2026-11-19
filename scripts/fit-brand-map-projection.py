#!/usr/bin/env python3
"""Fit equal-scale orthographic projection to reference land mask (maximize IoU)."""

from __future__ import annotations

import json
import math
import urllib.request
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT / "branding" / "source" / "flightvis-logo-final.png"
NE_PATH = ROOT / "scripts/data/ne_110m_land.geojson"
NE_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_110m_land.geojson"
)
FIT_DECIMATE = 1
OUT_PARAMS = ROOT / "assets/brand/projection.fit.json"
MAP_X, MAP_Y, MAP_W, MAP_H = 72, 88, 880, 848
MASK_W, MASK_H = 220, 212  # downsampled map crop for fast IoU


def download_land() -> None:
    NE_PATH.parent.mkdir(parents=True, exist_ok=True)
    if not NE_PATH.exists() or NE_PATH.stat().st_size < 10_000:
        urllib.request.urlretrieve(NE_URL, NE_PATH)


def ortho_xy(lon: float, lat: float, lon0_deg: float, lat0_deg: float) -> tuple[float, float]:
    lam = math.radians(lon)
    phi = math.radians(lat)
    lon0 = math.radians(lon0_deg)
    lat0 = math.radians(lat0_deg)
    x = math.cos(phi) * math.sin(lam - lon0)
    y = math.cos(lat0) * math.sin(phi) - math.sin(lat0) * math.cos(phi) * math.cos(lam - lon0)
    return x, y


def project(
    lon: float, lat: float, lon0: float, lat0: float, scale: float, ox: float, oy: float
) -> tuple[float, float]:
    gx, gy = ortho_xy(lon, lat, lon0, lat0)
    return ox + scale * gx, oy - scale * gy


def load_rings() -> list[list[tuple[float, float]]]:
    download_land()
    with NE_PATH.open(encoding="utf-8") as fh:
        gj = json.load(fh)
    rings: list[list[tuple[float, float]]] = []
    for feat in gj.get("features", []):
        geom = feat.get("geometry") or {}
        gtype = geom.get("type")
        polys: list = []
        if gtype == "Polygon":
            polys.append(geom["coordinates"])
        elif gtype == "MultiPolygon":
            polys.extend(geom["coordinates"])
        else:
            continue
        for poly in polys:
            for ring in poly:
                if len(ring) >= 4 and -175 <= ring[0][0] <= 45 and -58 <= ring[0][1] <= 85:
                    pts = [(float(lo), float(la)) for lo, la in ring]
                    if FIT_DECIMATE > 1:
                        pts = pts[::FIT_DECIMATE]
                    rings.append(pts)
    return rings


def reference_land_mask() -> np.ndarray:
    ref = np.array(Image.open(REF).convert("RGB"))
    r, g, b = ref[..., 0], ref[..., 1], ref[..., 2]
    magenta = (r > 150) & (b > 140) & (g < 130)
    white = (r > 205) & (g > 205) & (b > 205)
    land = (g > 150) & (g > r + 25) & (g > b + 45) & (r < 145) & ~magenta & ~white
    crop = land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    im = Image.fromarray((crop.astype(np.uint8)) * 255)
    im = im.resize((MASK_W, MASK_H), Image.Resampling.NEAREST)
    return np.array(im) > 127


def raster_mask(
    lon0: float,
    lat0: float,
    scale: float,
    ox: float,
    oy: float,
    rings: list[list[tuple[float, float]]],
) -> np.ndarray:
    # Full 1024 canvas, then crop — matches SVG space.
    img = Image.new("L", (1024, 1024), 0)
    draw = ImageDraw.Draw(img)
    for ring in rings:
        pts = [project(lo, la, lon0, lat0, scale, ox, oy) for lo, la in ring]
        if len(pts) < 3:
            continue
        draw.polygon(pts, fill=255)
    crop = img.crop((MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H))
    crop = crop.resize((MASK_W, MASK_H), Image.Resampling.NEAREST)
    return np.array(crop) > 127


def iou(a: np.ndarray, b: np.ndarray) -> float:
    inter = np.logical_and(a, b).sum()
    union = np.logical_or(a, b).sum()
    return float(inter) / float(union) if union else 0.0


def mask_centroid(mask: np.ndarray) -> tuple[float, float]:
    ys, xs = np.where(mask)
    if len(xs) == 0:
        return mask.shape[1] / 2, mask.shape[0] / 2
    return float(xs.mean()), float(ys.mean())


def align_offset(
    lon0: float,
    lat0: float,
    scale: float,
    rings: list[list[tuple[float, float]]],
    ref_mask_lo: np.ndarray,
) -> tuple[float, float, float]:
    """Return ox, oy, iou at equal-scale orthographic with centroid alignment."""
    # centroid in full canvas for reference
    rcx, rcy = mask_centroid(ref_mask_lo)
    map_rcx = MAP_X + rcx * (MAP_W / ref_mask_lo.shape[1])
    map_rcy = MAP_Y + rcy * (MAP_H / ref_mask_lo.shape[0])

    gxs: list[float] = []
    gys: list[float] = []
    for ring in rings:
        for lon, lat in ring:
            gx, gy = ortho_xy(lon, lat, lon0, lat0)
            gxs.append(gx)
            gys.append(gy)
    if not gxs:
        return MAP_X + MAP_W / 2, MAP_Y + MAP_H / 2, 0.0
    cx, cy = sum(gxs) / len(gxs), sum(gys) / len(gys)
    ox = map_rcx - scale * cx
    oy = map_rcy + scale * cy
    pred = raster_mask(lon0, lat0, scale, ox, oy, rings)
    return ox, oy, iou(pred, ref_mask_lo)


def estimate_offset_scale(
    lon0: float, lat0: float, rings: list[list[tuple[float, float]]]
) -> tuple[float, float, float]:
    xs: list[float] = []
    ys: list[float] = []
    for ring in rings:
        for lo, la in ring[:: max(1, len(ring) // 12)]:
            gx, gy = ortho_xy(lo, la, lon0, lat0)
            xs.append(gx)
            ys.append(gy)
    if not xs:
        return 520.0, 512.0, 400.0
    x0, x1 = min(xs), max(xs)
    y0, y1 = min(ys), max(ys)
    scale = min(MAP_W / (x1 - x0 + 1e-6), MAP_H / (y1 - y0 + 1e-6)) * 0.92
    cx = (x0 + x1) * 0.5
    cy = (y0 + y1) * 0.5
    ox = MAP_X + MAP_W * 0.5 - scale * cx
    oy = MAP_Y + MAP_H * 0.5 + scale * cy
    return scale, ox, oy


def refine(
    lon0: float,
    lat0: float,
    scale: float,
    ox: float,
    oy: float,
    ref_mask: np.ndarray,
    rings: list[list[tuple[float, float]]],
) -> tuple[float, float, float, float, float, float]:
    best = (lon0, lat0, scale, ox, oy, iou(raster_mask(lon0, lat0, scale, ox, oy, rings), ref_mask))
    for _ in range(6):
        improved = False
        for dlon in (-2.0, -1.0, 0.0, 1.0, 2.0):
            for dlat in (-2.0, -1.0, 0.0, 1.0, 2.0):
                l0, la0 = lon0 + dlon, lat0 + dlat
                sc, ox0, oy0 = estimate_offset_scale(l0, la0, rings)
                for dsl in (-0.08, -0.04, 0.0, 0.04, 0.08):
                    for dox in (-24.0, -12.0, 0.0, 12.0, 24.0):
                        for doy in (-24.0, -12.0, 0.0, 12.0, 24.0):
                            sc2, ox2, oy2 = sc * (1.0 + dsl), ox0 + dox, oy0 + doy
                            score = iou(raster_mask(l0, la0, sc2, ox2, oy2, rings), ref_mask)
                            if score > best[5]:
                                best = (l0, la0, sc2, ox2, oy2, score)
                                improved = True
        if not improved:
            break
        lon0, lat0, scale, ox, oy, _ = best
    return best


def main() -> None:
    if not REF.exists():
        raise SystemExit(f"missing reference {REF}")
    ref_mask = reference_land_mask()
    rings = load_rings()

    coarse_best = (-1.0, -80.0, 15.0, 400.0, 512.0, 512.0)
    for lon0 in np.arange(-90, -64, 2.0):
        for lat0 in np.arange(4, 24, 2.0):
            sc, ox, oy = estimate_offset_scale(float(lon0), float(lat0), rings)
            score = iou(raster_mask(float(lon0), float(lat0), sc, ox, oy, rings), ref_mask)
            if score > coarse_best[0]:
                coarse_best = (score, float(lon0), float(lat0), sc, ox, oy)

    _, lon0, lat0, scale, ox, oy = coarse_best
    lon0, lat0, scale, ox, oy, _ = refine(lon0, lat0, scale, ox, oy, ref_mask, rings)

    lon0, lat0, scale, ox, oy, _ = refine(lon0, lat0, scale, ox, oy, ref_mask, rings)

    try:
        from scipy.optimize import differential_evolution

        def neg_iou(params: list[float]) -> float:
            l0, la0, log_s, ox_p, oy_p = params
            return -iou(raster_mask(l0, la0, math.exp(log_s), ox_p, oy_p, rings), ref_mask)

        bounds = [
            (-92.0, -64.0),
            (4.0, 26.0),
            (math.log(340.0), math.log(620.0)),
            (float(MAP_X - 80), float(MAP_X + MAP_W + 80)),
            (float(MAP_Y - 40), float(MAP_Y + MAP_H + 40)),
        ]

        def clamp(v: float, lo: float, hi: float) -> float:
            return max(lo, min(hi, v))

        x0 = [
            clamp(lon0, *bounds[0]),
            clamp(lat0, *bounds[1]),
            clamp(math.log(scale), *bounds[2]),
            clamp(ox, *bounds[3]),
            clamp(oy, *bounds[4]),
        ]
        res = differential_evolution(
            neg_iou,
            bounds,
            x0=x0,
            maxiter=50,
            popsize=10,
            tol=1e-3,
            polish=True,
            seed=7,
        )
        lon0, lat0 = float(res.x[0]), float(res.x[1])
        scale, ox, oy = math.exp(res.x[2]), float(res.x[3]), float(res.x[4])
        lon0, lat0, scale, ox, oy, _ = refine(lon0, lat0, scale, ox, oy, ref_mask, rings)
    except ImportError:
        pass

    full_iou = iou(
        raster_mask(lon0, lat0, scale, ox, oy, rings),
        ref_mask,
    )
    # High-res IoU @ map crop full resolution
    full_a = raster_mask(lon0, lat0, scale, ox, oy, rings)
    full_ref = reference_land_mask()
    # re-raster at higher res for report
    img = Image.new("L", (1024, 1024), 0)
    draw = ImageDraw.Draw(img)
    for ring in rings:
        pts = [project(lo, la, lon0, lat0, scale, ox, oy) for lo, la in ring]
        if len(pts) >= 3:
            draw.polygon(pts, fill=255)
    crop = np.array(img.crop((MAP_X, MAP_Y, MAP_X + MAP_W, MAP_Y + MAP_H))) > 127
    ref_full = np.array(Image.open(REF).convert("RGB"))
    r, g, b = ref_full[..., 0], ref_full[..., 1], ref_full[..., 2]
    magenta = (r > 150) & (b > 140) & (g < 130)
    white = (r > 205) & (g > 205) & (b > 205)
    land = (g > 150) & (g > r + 25) & (g > b + 45) & (r < 145) & ~magenta & ~white
    ref_crop = land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    hi_iou = iou(crop, ref_crop)

    payload = {
        "lon0": round(lon0, 4),
        "lat0": round(lat0, 4),
        "scale": round(scale, 4),
        "ox": round(ox, 3),
        "oy": round(oy, 3),
        "iou_fast": round(full_iou, 4),
        "iou_full": round(hi_iou, 4),
    }
    OUT_PARAMS.parent.mkdir(parents=True, exist_ok=True)
    OUT_PARAMS.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    print(json.dumps(payload, indent=2))


if __name__ == "__main__":
    main()