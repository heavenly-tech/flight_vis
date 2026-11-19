"""Orthographic visibility clip (visible hemisphere) and antimeridian splits."""

from __future__ import annotations

import math


def _unit(lon: float, lat: float) -> tuple[float, float, float]:
    lam = math.radians(lon)
    phi = math.radians(lat)
    return math.cos(phi) * math.cos(lam), math.cos(phi) * math.sin(lam), math.sin(phi)


def _center(lon0: float, lat0: float) -> tuple[float, float, float]:
    return _unit(lon0, lat0)


def dot_visible(lon: float, lat: float, lon0: float, lat0: float) -> float:
    ux, uy, uz = _unit(lon, lat)
    cx, cy, cz = _center(lon0, lat0)
    return ux * cx + uy * cy + uz * cz


def visible(lon: float, lat: float, lon0: float, lat0: float, margin: float = 1e-5) -> bool:
    return dot_visible(lon, lat, lon0, lat0) > margin


def _lerp_lonlat(
    a: tuple[float, float], b: tuple[float, float], t: float
) -> tuple[float, float]:
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def _horizon_t(
    a: tuple[float, float],
    b: tuple[float, float],
    lon0: float,
    lat0: float,
) -> float:
    lo, hi = 0.0, 1.0
    da = dot_visible(a[0], a[1], lon0, lat0)
    for _ in range(28):
        mid = (lo + hi) * 0.5
        p = _lerp_lonlat(a, b, mid)
        dm = dot_visible(p[0], p[1], lon0, lat0)
        if da * dm >= 0:
            lo = mid
        else:
            hi = mid
    return (lo + hi) * 0.5


def _subdivide_edge(
    a: tuple[float, float],
    b: tuple[float, float],
    *,
    max_deg: float = 2.5,
) -> list[tuple[float, float]]:
    dlon = abs(b[0] - a[0])
    dlat = abs(b[1] - a[1])
    n = max(1, int(math.ceil(max(dlon, dlat) / max_deg)))
    if n == 1:
        return [a, b]
    out = [a]
    for i in range(1, n):
        t = i / n
        out.append(_lerp_lonlat(a, b, t))
    out.append(b)
    return out


def densify_ring(
    ring: list[tuple[float, float]], *, max_deg: float = 2.5
) -> list[tuple[float, float]]:
    closed = len(ring) >= 2 and ring[0] == ring[-1]
    pts = ring[:-1] if closed else ring[:]
    if len(pts) < 2:
        return ring[:]
    dense: list[tuple[float, float]] = []
    for i, a in enumerate(pts):
        b = pts[(i + 1) % len(pts)]
        for j, p in enumerate(_subdivide_edge(a, b, max_deg=max_deg)):
            if j == 0 and dense:
                continue
            dense.append(p)
    if closed:
        dense.append(dense[0])
    return dense


def _cross(a: tuple[float, float, float], b: tuple[float, float, float]) -> tuple[float, float, float]:
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _dot(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _norm(v: tuple[float, float, float]) -> tuple[float, float, float]:
    m = math.hypot(v[0], math.hypot(v[1], v[2]))
    if m < 1e-15:
        return (0.0, 0.0, 1.0)
    return (v[0] / m, v[1] / m, v[2] / m)


def _lonlat_from_unit(v: tuple[float, float, float]) -> tuple[float, float]:
    lon = math.degrees(math.atan2(v[1], v[0]))
    lat = math.degrees(math.asin(max(-1.0, min(1.0, v[2]))))
    return lon, lat


def _slerp(a: tuple[float, float, float], b: tuple[float, float, float], t: float) -> tuple[float, float, float]:
    d = max(-1.0, min(1.0, _dot(a, b)))
    omega = math.acos(d)
    if omega < 1e-8:
        return _norm((a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t))
    s1 = math.sin((1 - t) * omega) / math.sin(omega)
    s2 = math.sin(t * omega) / math.sin(omega)
    return _norm((a[0] * s1 + b[0] * s2, a[1] * s1 + b[1] * s2, a[2] * s1 + b[2] * s2))


def _project_horizon(v: tuple[float, float, float], c: tuple[float, float, float]) -> tuple[float, float, float]:
    """Point on unit sphere horizon circle (dot with c == 0)."""
    along = _dot(v, c)
    return _norm((v[0] - along * c[0], v[1] - along * c[1], v[2] - along * c[2]))


def horizon_arc(
    a: tuple[float, float],
    b: tuple[float, float],
    lon0: float,
    lat0: float,
    *,
    segments: int = 14,
) -> list[tuple[float, float]]:
    """Visible rim arc between two horizon points (lon/lat)."""
    c = _center(lon0, lat0)
    ua = _project_horizon(_unit(a[0], a[1]), c)
    ub = _project_horizon(_unit(b[0], b[1]), c)

    def arc_t(t0: float, t1: float) -> list[tuple[float, float]]:
        out: list[tuple[float, float]] = []
        for i in range(segments + 1):
            t = t0 + (t1 - t0) * (i / segments)
            p = _slerp(ua, ub, t)
            if dot_visible(*_lonlat_from_unit(p), lon0, lat0) > -0.02:
                out.append(_lonlat_from_unit(p))
        return out

    full = arc_t(0.0, 1.0)
    mid = _slerp(ua, ub, 0.5)
    if dot_visible(*_lonlat_from_unit(mid), lon0, lat0) <= 0:
        return arc_t(1.0, 0.0)[1:]  # other direction (skip duplicate start)
    return full[1:]


def close_visible_segments(
    open_chain: list[tuple[float, float]],
    lon0: float,
    lat0: float,
) -> list[tuple[float, float]]:
    if len(open_chain) < 3:
        return []
    rim = horizon_arc(open_chain[-1], open_chain[0], lon0, lat0)
    closed = open_chain[:] + rim
    return closed if len(closed) >= 3 else []


def clip_ring_visible(
    ring: list[tuple[float, float]],
    lon0: float,
    lat0: float,
) -> list[list[tuple[float, float]]]:
    """Return closed lon/lat rings on the visible hemisphere."""
    if len(ring) < 3:
        return []
    ring = densify_ring(ring, max_deg=2.0)
    closed = len(ring) >= 2 and abs(ring[0][0] - ring[-1][0]) < 1e-9 and abs(ring[0][1] - ring[-1][1]) < 1e-9
    pts = ring[:-1] if closed else ring[:]
    open_chains: list[list[tuple[float, float]]] = []
    current: list[tuple[float, float]] = []
    n = len(pts)
    for i in range(n):
        a = pts[i]
        b = pts[(i + 1) % n]
        va = visible(a[0], a[1], lon0, lat0)
        vb = visible(b[0], b[1], lon0, lat0)
        if va:
            if not current or current[-1] != a:
                current.append(a)
        if va != vb:
            t = _horizon_t(a, b, lon0, lat0)
            p = _lerp_lonlat(a, b, t)
            if va:
                current.append(p)
                open_chains.append(current)
                current = []
            else:
                current = [p]
    if len(current) >= 2:
        open_chains.append(current)

    out: list[list[tuple[float, float]]] = []
    if closed and all(visible(x[0], x[1], lon0, lat0, margin=-1e-6) for x in pts):
        out.append(ring if ring[0] == ring[-1] else ring + [ring[0]])
        return out
    for chain in open_chains:
        poly = close_visible_segments(chain, lon0, lat0)
        if len(poly) >= 3:
            out.append(poly)
    return out


def _unwrap_ring(ring: list[tuple[float, float]], lon0: float) -> list[tuple[float, float]]:
    out: list[tuple[float, float]] = []
    for i, (lon, lat) in enumerate(ring):
        if i == 0:
            out.append((lon, lat))
            continue
        prev = out[-1][0]
        dlon = lon - prev
        while dlon > 180:
            lon -= 360
            dlon = lon - prev
        while dlon < -180:
            lon += 360
            dlon = lon - prev
        out.append((lon, lat))
    return out


def split_antimeridian(
    ring: list[tuple[float, float]], lon0: float
) -> list[list[tuple[float, float]]]:
    """Split when consecutive longitudes jump > 180° (relative to unwrapped chain)."""
    pts = _unwrap_ring(ring, lon0)
    if len(pts) < 3:
        return []
    segments: list[list[tuple[float, float]]] = []
    current: list[tuple[float, float]] = [pts[0]]
    for i in range(1, len(pts)):
        lon, lat = pts[i]
        prev_lon = current[-1][0]
        if abs(lon - prev_lon) > 180:
            if len(current) >= 3:
                segments.append(current)
            current = [(lon, lat)]
        else:
            current.append((lon, lat))
    if len(current) >= 3:
        segments.append(current)
    return segments if segments else [pts]


def prepare_rings(
    ring: list[tuple[float, float]],
    lon0: float,
    lat0: float,
    *,
    drop_antarctica_below: float = -55.0,
) -> list[list[tuple[float, float]]]:
    if all(la < drop_antarctica_below for _, la in ring):
        return []
    out: list[list[tuple[float, float]]] = []
    for clipped in clip_ring_visible(ring, lon0, lat0):
        for seg in split_antimeridian(clipped, lon0):
            if any(la >= drop_antarctica_below for _, la in seg):
                out.append(seg)
    return out