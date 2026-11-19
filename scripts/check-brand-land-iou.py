#!/usr/bin/env python3
"""Verify chart land vector IoU vs branding/source/ref-land-mask.png (fail if < 0.93)."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
EXTRACT = ROOT / "branding/source/extract-chart-vector.py"
MASK = ROOT / "branding/source/ref-land-mask.png"
VEC = ROOT / "assets/brand/chart-vector.json"
MIN_IOU = 0.93


def raster(paths: list[str]) -> np.ndarray:
    img = Image.new("L", (1024, 1024), 0)
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


def iou(a: np.ndarray, b: np.ndarray) -> float:
    u = np.logical_or(a, b).sum()
    return float(np.logical_and(a, b).sum()) / float(u) if u else 0.0


def main() -> int:
    subprocess.run([sys.executable, str(EXTRACT)], cwd=ROOT, check=True)
    ref = np.array(Image.open(MASK).convert("L")) > 127
    data = json.loads(VEC.read_text(encoding="utf-8"))
    pred = raster(data["land_paths"])
    score = iou(pred, ref)
    print(
        json.dumps(
            {
                "land_iou_ref_mask": round(score, 4),
                "land_pixels_raster": int(pred.sum()),
                "land_pixels_ref": int(ref.sum()),
            },
            indent=2,
        )
    )
    if score < MIN_IOU:
        print(f"FAIL: IoU {score:.4f} < {MIN_IOU}", file=sys.stderr)
        return 1
    print(f"OK: land IoU {score:.4f} >= {MIN_IOU}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())