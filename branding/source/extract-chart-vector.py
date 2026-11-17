#!/usr/bin/env python3
"""Vectorise owner-approved map layers from chart.jpg (paths relative to this file)."""

from __future__ import annotations

import json
from pathlib import Path

import cv2
import numpy as np
from PIL import Image
from scipy.ndimage import label as nd_label
from skimage.morphology import remove_small_holes, remove_small_objects

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CHART = HERE / "chart.jpg"
REF_LAND_MASK = HERE / "ref-land-mask.png"
OUT = ROOT / "assets" / "brand" / "chart-vector.json"

MAP_X, MAP_Y, MAP_W, MAP_H, MAP_RX = 72, 88, 880, 848, 56
LAND_COLOR = "#6DB33F"
GRID_COLOR = "#0B3378"
BORDER_COLOR = "#0D3A8C"
HOLE_AREA = 30
MIN_ISLAND = 30


def fmt(v: float) -> str:
    s = f"{v:.1f}".rstrip("0").rstrip(".")
    return s if s != "-0" else "0"


def path_from_contour(contour: np.ndarray, eps: float) -> str:
    if len(contour) < 3:
        return ""
    approx = cv2.approxPolyDP(contour, eps, True)
    if len(approx) < 3:
        approx = contour
    pts = [(float(p[0][0]), float(p[0][1])) for p in approx]
    return (
        f"M {fmt(pts[0][0])},{fmt(pts[0][1])}"
        + "".join(f" L {fmt(x)},{fmt(y)}" for x, y in pts[1:])
        + " Z"
    )


def land_mask_rgb(chart: np.ndarray) -> np.ndarray:
    """Owner rule on native 1024 chart: land = (G > R+25) & (G > B+15)."""
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    land = (g > r + 25) & (g > b + 15)
    land = remove_small_holes(land, max_size=HOLE_AREA)
    land = remove_small_objects(land, max_size=MIN_ISLAND, connectivity=2)
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (3, 3))
    land = cv2.morphologyEx(land.astype(np.uint8) * 255, cv2.MORPH_CLOSE, k, 1) > 0
    return land


def load_reference_mask() -> np.ndarray:
    if REF_LAND_MASK.exists():
        m = np.array(Image.open(REF_LAND_MASK).convert("L"))
        return m > 127
    chart = np.array(Image.open(CHART).convert("RGB"))
    return land_mask_rgb(chart)


def raster_paths(paths: list[str], shape: tuple[int, int] = (1024, 1024)) -> np.ndarray:
    from PIL import ImageDraw

    img = Image.new("L", shape, 0)
    draw = ImageDraw.Draw(img)
    for d in paths:
        pts = []
        for tok in d.replace("M ", "").replace("Z", "").split(" L "):
            if "," not in tok:
                continue
            x, y = tok.split(",", 1)
            pts.append((float(x), float(y)))
        if len(pts) >= 3:
            draw.polygon(pts, fill=255)
    return np.array(img) > 127


def mask_iou(a: np.ndarray, b: np.ndarray) -> float:
    u = np.logical_or(a, b).sum()
    return float(np.logical_and(a, b).sum()) / float(u) if u else 0.0


def contours_to_paths(mask: np.ndarray, eps: float, min_area: float = 80.0) -> list[str]:
    paths: list[str] = []
    contours, _ = cv2.findContours(
        mask.astype(np.uint8) * 255, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_NONE
    )
    contours = sorted(contours, key=cv2.contourArea, reverse=True)
    for cnt in contours:
        if cv2.contourArea(cnt) < min_area:
            continue
        d = path_from_contour(cnt, eps)
        if d:
            paths.append(d)
    return paths


def graticule_paths(chart: np.ndarray, land: np.ndarray) -> list[str]:
    """Darker blue lines: B > G+60 and ~15–25% darker than local ocean."""
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    lum = (0.299 * r + 0.587 * g + 0.114 * b).astype(np.float32)
    ocean = ~land & (b > g + 60)
    ocean_f = ocean.astype(np.float32)
    denom = cv2.blur(ocean_f, (31, 31))
    num = cv2.blur(lum * ocean_f, (31, 31))
    local_lum = np.where(denom > 0.05, num / (denom + 1e-6), lum)
    rel_drop = (local_lum - lum) / np.maximum(local_lum, 1.0)
    grid = ocean & (b > g + 60) & (rel_drop >= 0.12) & (rel_drop <= 0.32)
    thin = cv2.Canny(grid.astype(np.uint8) * 255, 15, 55)
    thin = cv2.dilate(thin, np.ones((2, 2), np.uint8), 1)
    contours, _ = cv2.findContours(thin, cv2.RETR_LIST, cv2.CHAIN_APPROX_NONE)
    paths: list[str] = []
    for cnt in contours:
        if len(cnt) < 8:
            continue
        length = cv2.arcLength(cnt, False)
        if length < 35 or length > 2000:
            continue
        approx = cv2.approxPolyDP(cnt, 1.8, False)
        if len(approx) < 2:
            continue
        pts = [(float(p[0][0]), float(p[0][1])) for p in approx]
        if not all(MAP_X - 6 <= x <= MAP_X + MAP_W + 6 for x, _ in pts):
            continue
        if not all(MAP_Y - 6 <= y <= MAP_Y + MAP_H + 6 for _, y in pts):
            continue
        path = f"M {fmt(pts[0][0])},{fmt(pts[0][1])}"
        path += "".join(f" L {fmt(x)},{fmt(y)}" for x, y in pts[1:])
        paths.append(path)
    return paths


def ocean_colors(chart: np.ndarray, land: np.ndarray) -> tuple[str, str]:
    """Bright azure centre (chart) with modest edge darkening — not navy vignette."""
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    cx, cy = 512, 430
    ring = np.zeros(land.shape, dtype=bool)
    yy, xx = np.ogrid[: land.shape[0], : land.shape[1]]
    dist = np.hypot(xx - cx, yy - cy)
    ring |= (dist > 80) & (dist < 260)
    ocean = ring & ~land & (b > g) & (b > r)
    if ocean.sum() < 200:
        return "#4EB0FF", "#2490E8"
    px = chart[ocean]
    bright = px[np.argsort(px[:, 2] - px[:, 1])[-300:]]
    mid = px[np.argsort(px.mean(axis=1))[len(px) // 2 : len(px) // 2 + 100]]

    def hexpx(buf: np.ndarray) -> str:
        m = np.median(buf, axis=0).astype(int)
        return f"#{m[0]:02x}{m[1]:02x}{m[2]:02x}".upper()

    hi = hexpx(bright)
    lo = hexpx(mid)
    # Gradient center must be the lighter azure.
    if sum(int(hi[i : i + 2], 16) for i in (1, 3, 5)) < sum(int(lo[i : i + 2], 16) for i in (1, 3, 5)):
        hi, lo = lo, hi
    lum_hi = sum(int(hi[i : i + 2], 16) for i in (1, 3, 5))
    lum_lo = sum(int(lo[i : i + 2], 16) for i in (1, 3, 5))
    if lum_hi < lum_lo:
        hi, lo = lo, hi
    if int(hi[3:5], 16) < 170 or int(hi[5:7], 16) < 200:
        return "#4EB0FF", "#1E90FF"
    return hi, lo


def main() -> None:
    if not CHART.exists():
        raise SystemExit(f"missing {CHART}")
    chart = np.array(Image.open(CHART).convert("RGB"))
    if chart.shape[0] != 1024 or chart.shape[1] != 1024:
        raise SystemExit(f"chart.jpg must be 1024×1024, got {chart.shape[:2]}")
    land = land_mask_rgb(chart)
    ref_m = load_reference_mask()
    best_eps, best_iou, best_paths = 1.0, -1.0, []
    for eps in (0.6, 0.8, 1.0, 1.2, 1.5, 1.8, 2.0, 2.5):
        paths = contours_to_paths(land, eps)
        pred = raster_paths(paths)
        iou = mask_iou(pred, ref_m)
        if iou > best_iou:
            best_eps, best_iou, best_paths = eps, iou, paths
    small_paths = contours_to_paths(land, max(2.8, best_eps * 2.0))
    grids = graticule_paths(chart, land)
    hi, lo = ocean_colors(chart, land)
    pred = raster_paths(best_paths)
    payload = {
        "map": {"x": MAP_X, "y": MAP_Y, "w": MAP_W, "h": MAP_H, "rx": MAP_RX},
        "colors": {
            "land": LAND_COLOR,
            "grid": GRID_COLOR,
            "border": BORDER_COLOR,
            "ocean_hi": hi,
            "ocean_lo": lo,
        },
        "land_paths": best_paths,
        "land_paths_small": small_paths[: max(15, len(small_paths))],
        "graticule_paths": grids,
        "metrics": {
            "land_iou_ref_mask": round(best_iou, 4),
            "land_pixels_raster": int(pred.sum()),
            "land_pixels_ref": int(ref_m.sum()),
            "land_eps_full": best_eps,
            "graticule_paths": len(grids),
            "coast_source": "chart.jpg",
        },
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    print(json.dumps(payload["metrics"], indent=2))
    print(f"wrote {OUT}")
    if best_iou < 0.93:
        raise SystemExit(f"land IoU {best_iou:.4f} < 0.93 vs ref-land-mask.png")


if __name__ == "__main__":
    main()