#!/usr/bin/env python3
"""Render SVG and compare to reference PNG; write side-by-side and MAE stats."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT / "branding" / "source" / "flightvis-logo-final.png"
SVG = ROOT / "assets/brand/flightvis-logo.svg"
OUT_DIR = Path("/opt/cursor/artifacts")
RENDER = OUT_DIR / "pr-logo-render-1024.png"
COMPARE = OUT_DIR / "pr-logo-compare-1024.png"


def render_svg() -> None:
    subprocess.run(
        [
            "node",
            "-e",
            (
                "import fs from 'fs';import {Resvg} from '@resvg/resvg-js';"
                f"const svg=fs.readFileSync('{SVG}');"
                f"fs.writeFileSync('{RENDER}',new Resvg(svg,{{fitTo:{{mode:'width',value:1024}}}}).render().asPng());"
            ),
        ],
        cwd=ROOT,
        check=True,
    )


def main() -> int:
    if not REF.exists():
        print(f"missing reference {REF}", file=sys.stderr)
        return 1
    render_svg()
    import numpy as np
    from PIL import Image, ImageDraw, ImageFont

    ref = Image.open(REF).convert("RGBA")
    ren = Image.open(RENDER).convert("RGBA")
    if ref.size != ren.size:
        ren = ren.resize(ref.size, Image.Resampling.LANCZOS)

    r1 = np.asarray(ref.convert("RGB"), dtype=np.float32)
    r2 = np.asarray(ren.convert("RGB"), dtype=np.float32)
    mae = float(np.mean(np.abs(r1 - r2)))
    # Land-mask IoU (reference green vs render green)
    ref_full = np.array(Image.open(REF).convert("RGB"))
    R, G, B = ref_full[..., 0], ref_full[..., 1], ref_full[..., 2]
    magenta = (R > 150) & (B > 140) & (G < 130)
    white = (R > 205) & (G > 205) & (B > 205)
    MAP_X, MAP_Y, MAP_W, MAP_H = 72, 88, 880, 848
    ref_land = (
        (G > 150) & (G > R + 25) & (G > B + 45) & (R < 145) & ~magenta & ~white
    )
    ref_land = ref_land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    rR, rG, rB = ren.convert("RGB").split()
    rnp = np.asarray(ren.convert("RGB"))
    rR, rG, rB = rnp[..., 0], rnp[..., 1], rnp[..., 2]
    ren_land = (rG > 120) & (rG > rR + 10) & (rG > rB + 20)
    ren_land = ren_land[MAP_Y : MAP_Y + MAP_H, MAP_X : MAP_X + MAP_W]
    inter = np.logical_and(ref_land, ren_land).sum()
    union = np.logical_or(ref_land, ren_land).sum()
    land_iou = float(inter) / float(union) if union else 0.0
    side = Image.new("RGBA", (ref.width * 2 + 20, ref.height + 40), (18, 24, 38, 255))
    side.paste(ref, (10, 30))
    side.paste(ren, (ref.width + 20, 30))
    draw = ImageDraw.Draw(side)
    draw.text((16, 6), "reference", fill=(240, 248, 255))
    draw.text((ref.width + 26, 6), "SVG render", fill=(240, 248, 255))
    COMPARE.parent.mkdir(parents=True, exist_ok=True)
    side.save(COMPARE)
    stats = {
        "mae_rgb": round(mae, 2),
        "land_iou": round(land_iou, 4),
        "render": str(RENDER),
        "compare": str(COMPARE),
    }
    (OUT_DIR / "pr-logo-compare-stats.json").write_text(json.dumps(stats, indent=2))
    print(json.dumps(stats, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())