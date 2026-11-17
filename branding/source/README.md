# flight_vis branding source

Owner-approved raster primitives and the approved Pillow compose script. Vector assets under `assets/brand/` are generated to match this reference.

## Re-render the approved PNG

Requirements: Python 3, [Pillow](https://pypi.org/project/pillow/), [cairosvg](https://pypi.org/project/cairosvg/), [numpy](https://pypi.org/project/numpy/).

```bash
pip install pillow cairosvg numpy
cd branding/source
python3 flightvis-logo-final.py
```

Output: `flightvis-logo-final.png` (1024×1024).

### Decode the full owner bundle

If `branding-source.tar.gz.b64.txt` is provided:

```bash
base64 -d < branding-source.tar.gz.b64.txt | tar xzf - -C branding/source
# or: ./scripts/decode-branding-source.sh
```

Expected extras (when tarball present): `flightvis-logo-globe-track.jpg`, `history/track.py`, `history/square.py` … `history/square12.py`.

## Track (exact SVG path)

Built in script from tail→tip unit vector `u`:

- Cubic spine: `M150,885 C 330,895 590,850 640,740 C 680,655 660,640 590,640 C 400,640 212,630 212,520 C 205,195 {c2} {P}` then `L {T+4u}`.
- `T = (610, 425)`, `u = unit(tip − tail)` with `tail = (672, 450)`, `tip = (982, 118)` in plane **base** space (before plane transform).
- `P = T − 60·u`, `c2 = P − 100·u`.

Stroke (SVG overlay via CairoSVG):

| Property | Value |
|----------|-------|
| Color | `#FF00CC` |
| Width | `26` |
| Line cap | `butt` |
| Dash | `46 24` |

## Paper plane

1. **Clip polygon** on `base.jpg` (1024 canvas):  
   `(536,280), (982,118), (815,446), (745,398), (672,450), (650,328)`.
2. **Mask:** inside polygon AND (near-white OR navy: `(B>R+20) & (G<110) & (R<80)`), then max-filter + light blur.
3. **Transform:** scale `S=1.12` about tail; map tail `(672,450)` → `T (610,425)`; rotation keeps tail→tip angle (vector `(310,−332)`).

Vector SVG (`assets/brand/flightvis-logo*.svg`) uses the same polygon split (white wings, navy keel, white halo + navy stroke) and the same `translate(610,425) scale(1.12) translate(-672,-450)`.

## Chart base

- `chart.jpg` — square chart (ocean gradient, graticule, illustrated coastlines). Composited under track and plane.
- `base.jpg` — paper-plane source art for masking/transform.
- `flightvis-logo-globe-track.jpg` — earlier globe concept (track on globe); no track-free globe base shipped.

## Vector pipeline (`assets/brand/`)

| Step | Script |
|------|--------|
| Extract land + graticule + colours from `chart.jpg` | `branding/source/extract-chart-vector.py` |
| Compose SVGs (track/plane unchanged) | `scripts/compose-brand-svgs.py` |
| PNG icons / favicon | `npm run export:icons` |

Land is traced from **`chart.jpg`** (HSV green 70–140° + int16-safe RGB, morph open/close, specks &lt; 40 px dropped, `approxPolyDP` paths at 1024). IoU is reported against `flightvis-logo-final.png` green mask and the chart land mask.

Natural Earth / projection scripts under `scripts/` remain for experiments but are not used by `npm run brand:compose`.

Regenerate everything:

```bash
npm run brand:compose && npm run export:icons
```

QA: `python3 scripts/compare-brand-render.py` (side-by-side vs `flightvis-logo-final.png`).

`assets/brand/reference/` holds PR comparison assets (`side16b.png`) and may mirror files here; **canonical paths** are this directory.