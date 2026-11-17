#!/usr/bin/env python3
"""Vectorise owner-approved map layers from chart.jpg (paths relative to this file)."""

from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image
from scipy.ndimage import label as nd_label

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
CHART = HERE / "chart.jpg"
REF = HERE / "flightvis-logo-final.png"
OUT = ROOT / "assets" / "brand" / "chart-vector.json"

MAP_X, MAP_Y, MAP_W, MAP_H, MAP_RX = 72, 88, 880, 848, 56
LAND_COLOR = "#6DB33F"
GRID_COLOR = "#0B3378"
BORDER_COLOR = "#0D3A8C"
MIN_SPECK = 40
MIN_LAND_AREA = 5000


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


def open_close(mask: np.ndarray, ksz: int = 5) -> np.ndarray:
    k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (ksz, ksz))
    out = cv2.morphologyEx(mask.astype(np.uint8) * 255, cv2.MORPH_OPEN, k, 1)
    out = cv2.morphologyEx(out, cv2.MORPH_CLOSE, k, 3)
    return out > 127


def drop_specks(mask: np.ndarray, min_px: int = MIN_SPECK) -> np.ndarray:
    lbl, n = nd_label(mask)
    out = mask.copy()
    for i in range(1, n + 1):
        if (lbl == i).sum() < min_px:
            out[lbl == i] = False
    return out


def land_mask(chart: np.ndarray) -> np.ndarray:
    """HSV green (70–140°, S>0.25) with int16-safe green dominance."""
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    hsv = cv2.cvtColor(chart, cv2.COLOR_RGB2HSV)
    h_deg = hsv[..., 0].astype(np.float32) * 2.0
    sat = hsv[..., 1].astype(np.float32) / 255.0
    hsv_core = (h_deg >= 70) & (h_deg <= 140) & (sat > 0.25)
    rgb_core = (g > 150) & (g > r + 25) & (g > b + 45) & (r < 145)
    merged = rgb_core & (hsv_core | ((h_deg >= 36) & (h_deg <= 98) & (sat > 0.12)))
    if merged.sum() < 80_000:
        merged = rgb_core
    mask = open_close(merged)
    mask = drop_specks(mask)
    # Reject graticule specks and Pacific noise: drop tiny components in open ocean.
    lbl, n = nd_label(mask)
    crop = slice(MAP_Y, MAP_Y + MAP_H), slice(MAP_X, MAP_X + MAP_W)
    ocean_box = (slice(MAP_Y + 250, MAP_Y + 600), slice(MAP_X + 40, MAP_X + 220))
    out = mask.copy()
    for i in range(1, n + 1):
        comp = lbl == i
        if comp.sum() < MIN_SPECK:
            out[comp] = False
            continue
        oce = comp[ocean_box].sum()
        if oce > 0 and oce / comp.sum() > 0.55:
            out[comp] = False
    return out


def graticule_paths(chart: np.ndarray, land: np.ndarray) -> list[str]:
    """Extract thin darker-blue graticule strokes from chart ocean."""
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    grid = (
        ~land
        & (b > g + 12)
        & (b > r + 20)
        & (r < 112)
        & (g < 168)
        & (g > 48)
    )
    thin = cv2.Canny(grid.astype(np.uint8) * 255, 20, 70)
    thin = cv2.dilate(thin, np.ones((2, 2), np.uint8), 1)
    contours, _ = cv2.findContours(thin, cv2.RETR_LIST, cv2.CHAIN_APPROX_NONE)
    paths: list[str] = []
    for cnt in contours:
        if len(cnt) < 12:
            continue
        length = cv2.arcLength(cnt, False)
        if length < 50 or length > 1500:
            continue
        approx = cv2.approxPolyDP(cnt, 2.0, False)
        if len(approx) < 2:
            continue
        ys = [p[0][1] for p in approx]
        if max(ys) - min(ys) < 3 and len(approx) > 10:
            continue
        pts = [(float(p[0][0]), float(p[0][1])) for p in approx]
        if not all(MAP_X - 4 <= x <= MAP_X + MAP_W + 4 for x, _ in pts):
            continue
        if not all(MAP_Y - 4 <= y <= MAP_Y + MAP_H + 4 for _, y in pts):
            continue
        path = f"M {fmt(pts[0][0])},{fmt(pts[0][1])}"
        path += "".join(f" L {fmt(x)},{fmt(y)}" for x, y in pts[1:])
        paths.append(path)
    return paths


def sample_ocean_colors(chart: np.ndarray, land: np.ndarray) -> tuple[str, str]:
    r, g, b = (
        chart[..., 0].astype(np.int16),
        chart[..., 1].astype(np.int16),
        chart[..., 2].astype(np.int16),
    )
    inner = np.zeros(land.shape, dtype=bool)
    inner[MAP_Y + 60 : MAP_Y + MAP_H - 60, MAP_X + 60 : MAP_X + MAP_W - 60] = True
    ocean = (
        inner
        & ~land
        & (b > g + 12)
        & (b > r + 8)
        & (g < 185)
        & (r < 140)
    )
    if ocean.sum() < 500:
        return "#4EB0FF", "#156BBF"
    px = chart[ocean]
    bright = px[np.argsort(px.mean(axis=1))[-400:]]
    dark = px[np.argsort(px.mean(axis=1))[:400]]
    def hexpx(buf: np.ndarray) -> str:
        m = np.median(buf, axis=0).astype(int)
        return f"#{m[0]:02x}{m[1]:02x}{m[2]:02x}".upper()

    return hexpx(bright), hexpx(dark)


def reference_land_mask(ref: np.ndarray) -> np.ndarray:
    r, g, b = (
        ref[..., 0].astype(np.int16),
        ref[..., 1].astype(np.int16),
        ref[..., 2].astype(np.int16),
    )
    magenta = (r > 150) & (b > 140) & (g < 130)
    white = (r > 205) & (g > 205) & (b > 205)
    return (
        (g > 150)
        & (g > r + 25)
        & (g > b + 45)
        & (r < 145)
        & ~magenta
        & ~white
    )


def raster_paths(paths: list[str], shape: tuple[int, int] = (1024, 1024)) -> np.ndarray:
    from PIL import ImageDraw

    img = Image.new("L", shape, 0)
    draw = ImageDraw.Draw(img)
    for d in paths:
        pts = []
        tokens = d.replace("M ", "").split(" L ")
        for tok in tokens:
            if tok.endswith(" Z"):
                tok = tok[:-2]
            if "," not in tok:
                continue
            x, y = tok.split(",", 1)
            pts.append((float(x), float(y)))
        if len(pts) >= 3:
            draw.polygon(pts, fill=255)
    crop = np.array(img)[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W] > 127
    return crop


def mask_iou(a: np.ndarray, b: np.ndarray) -> float:
    u = np.logical_or(a, b).sum()
    return float(np.logical_and(a, b).sum()) / float(u) if u else 0.0


def contours_to_paths(mask: np.ndarray, eps: float) -> list[str]:
    paths: list[str] = []
    contours, _ = cv2.findContours(
        mask.astype(np.uint8) * 255, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_NONE
    )
    contours = sorted(contours, key=cv2.contourArea, reverse=True)
    for cnt in contours:
        if cv2.contourArea(cnt) < MIN_LAND_AREA:
            continue
        d = path_from_contour(cnt, eps)
        if d:
            paths.append(d)
    return paths


def main() -> None:
    if not CHART.exists():
        raise SystemExit(f"missing {CHART}")
    chart = np.array(Image.open(CHART).convert("RGB"))
    land = land_mask(chart)
    best_eps, best_key, best_paths = 1.2, -1.0, []
    chart_m = land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    for eps in (1.0, 1.2, 1.5, 1.8, 2.2, 2.8, 3.5):
        paths = contours_to_paths(land, eps)
        if not paths:
            continue
        pred = raster_paths(paths)
        self_iou = mask_iou(pred, chart_m)
        ref_score = self_iou
        if REF.exists():
            ref = np.array(Image.open(REF).convert("RGB"))
            ref_m = reference_land_mask(ref)[
                MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W
            ]
            ref_score = mask_iou(pred, ref_m)
        key = self_iou if self_iou >= 0.9 else ref_score
        if key > best_key:
            best_eps, best_key, best_paths = eps, key, paths
    small_paths = contours_to_paths(land, max(3.5, best_eps * 2.8))
    try:
        grids = graticule_paths(chart, land)
    except Exception:
        grids = []
    if not grids:
        grids = []
    hi, lo = sample_ocean_colors(chart, land)
    ref_iou = 0.0
    chart_iou = 0.0
    if REF.exists() and best_paths:
        from scipy.ndimage import binary_dilation

        ref = np.array(Image.open(REF).convert("RGB"))
        ref_m = reference_land_mask(ref)[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
        pred = raster_paths(best_paths)
        ref_iou = mask_iou(pred, ref_m)
        chart_iou = mask_iou(pred, land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W])
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
        "land_paths_small": small_paths[: max(12, len(small_paths) // 2)],
        "graticule_paths": grids,
        "metrics": {
            "land_iou_ref": round(ref_iou, 4),
            "land_iou_chart_mask": round(chart_iou, 4),
            "land_eps_full": best_eps,
            "coast_source": "chart.jpg",
        },
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    print(json.dumps(payload["metrics"], indent=2))
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()