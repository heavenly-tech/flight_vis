/**
 * Live aircraft trail settings, pure windowing/fade math, and 2D canvas + metadata overlay.
 */

export const TRAIL_OFF = 0 as const;
export const TRAIL_ALL = -1 as const;
export const DEFAULT_TRAIL_MINUTES = 5 as const;
export const MAX_TRAIL_WINDOW_MIN = 120 as const;

/** Canonical ordered allowed trail values (Off, 2..30 min, All). Must stay aligned with Rust trail.rs. */
export const ALLOWED_TRAIL_MINUTES = [0, 2, 5, 10, 15, 30, -1] as const;
export type TrailMinutes = (typeof ALLOWED_TRAIL_MINUTES)[number];

/** `[t_ms, lat, lon]` tuple matching `/api/admin/map/trails` and `/api/display/map/trails`. */
export type TrailPoint = readonly [number, number, number];

export type FlightTrail = {
  flightId: string;
  tail: string;
  tailSource?: "club" | "pilot";
  trailMinutes: TrailMinutes;
  nonClub?: boolean;
  points: TrailPoint[];
};

export type ScreenPoint = { x: number; y: number };

export type TrailSegment = {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  alpha: number;
};

export type LiveFixHint = {
  t?: number;
  lat: number;
  lon: number;
};

export type RenderTrailItem = {
  flightId: string;
  tail: string;
  tailSource?: "club" | "pilot";
  trailMinutes: TrailMinutes;
  nonClub?: boolean;
  points: readonly TrailPoint[];
  rgb: readonly [number, number, number];
};

/** Default aircraft marker colour (#ff5500). */
export const DEFAULT_TRAIL_RGB: readonly [number, number, number] = [255, 85, 0];
/** Focused kiosk aircraft marker colour (sky blue matching #0284c7 / #67e8f9). */
export const FOCUSED_TRAIL_RGB: readonly [number, number, number] = [56, 189, 248];
/** Desaturated slate-grey trail colour for nearby public flights from other clubs. */
export const NON_CLUB_TRAIL_RGB: readonly [number, number, number] = [148, 163, 184];

const ALLOWED_SET = new Set<number>(ALLOWED_TRAIL_MINUTES);
const MS_PER_MIN = 60_000;

export function isValidTrailMinutes(value: unknown): value is TrailMinutes {
  return typeof value === "number" && Number.isInteger(value) && ALLOWED_SET.has(value);
}

export function parseTrailMinutes(
  value: unknown,
  fallback: TrailMinutes = DEFAULT_TRAIL_MINUTES,
): TrailMinutes {
  return isValidTrailMinutes(value) ? value : fallback;
}

export function parseTrailChipAttribute(raw: string | null | undefined): TrailMinutes | null {
  if (typeof raw !== "string" || raw.trim() === "") return null;
  const n = Number(raw);
  return isValidTrailMinutes(n) ? n : null;
}

export function trailMinutesWindowMs(minutes: TrailMinutes): number | null {
  if (minutes === TRAIL_OFF) return null;
  if (minutes === TRAIL_ALL) return MAX_TRAIL_WINDOW_MIN * MS_PER_MIN;
  return minutes * MS_PER_MIN;
}

export function trailOptionI18n(value: TrailMinutes): {
  key: string;
  params?: Record<string, string>;
} {
  if (value === TRAIL_OFF) return { key: "trail.off" };
  if (value === TRAIL_ALL) return { key: "trail.all" };
  return { key: "trail.minutes", params: { minutes: String(value) } };
}

function isValidCoord(lat: unknown, lon: unknown): boolean {
  return (
    typeof lat === "number" &&
    typeof lon === "number" &&
    Number.isFinite(lat) &&
    Number.isFinite(lon) &&
    lat >= -90 &&
    lat <= 90 &&
    lon >= -180 &&
    lon <= 180
  );
}

export function parseTrailsPayload(raw: unknown): FlightTrail[] {
  if (!raw || typeof raw !== "object") return [];
  const trails = (raw as { trails?: unknown }).trails;
  if (!Array.isArray(trails)) return [];

  const out: FlightTrail[] = [];
  for (const item of trails) {
    if (!item || typeof item !== "object") continue;
    const rec = item as {
      flightId?: unknown;
      tail?: unknown;
      tailSource?: unknown;
      trailMinutes?: unknown;
      nonClub?: unknown;
      points?: unknown;
    };
    const flightId = typeof rec.flightId === "string" ? rec.flightId.trim() : "";
    const tail = typeof rec.tail === "string" ? rec.tail.trim() : "";
    if (!tail) continue;
    const trailMinutes = parseTrailMinutes(rec.trailMinutes, DEFAULT_TRAIL_MINUTES);
    const points: TrailPoint[] = [];
    if (Array.isArray(rec.points)) {
      for (const p of rec.points) {
        if (!Array.isArray(p) || p.length < 3) continue;
        const [tMs, lat, lon] = p;
        if (typeof tMs !== "number" || !Number.isFinite(tMs)) continue;
        if (!isValidCoord(lat, lon)) continue;
        points.push([tMs, lat as number, lon as number]);
      }
    }
    const entry: FlightTrail = { flightId, tail, trailMinutes, points };
    if (rec.tailSource === "club" || rec.tailSource === "pilot") {
      entry.tailSource = rec.tailSource;
    }
    if (rec.nonClub === true) {
      entry.nonClub = true;
    }
    out.push(entry);
  }
  return out;
}

/**
 * Pure helper: appends the live marker position so the trail stays attached between polls,
 * then trims older points to the effective trail window relative to the newest point.
 */
export function prepareTrailPoints(
  points: readonly TrailPoint[],
  trailMinutes: TrailMinutes,
  liveFix?: LiveFixHint | null,
): TrailPoint[] {
  const windowMs = trailMinutesWindowMs(trailMinutes);
  if (windowMs === null) return [];

  const sorted: TrailPoint[] = [];
  for (const p of points) {
    if (typeof p[0] === "number" && Number.isFinite(p[0]) && isValidCoord(p[1], p[2])) {
      sorted.push([p[0], p[1], p[2]]);
    }
  }
  sorted.sort((a, b) => a[0] - b[0]);

  if (liveFix && isValidCoord(liveFix.lat, liveFix.lon)) {
    const last = sorted.length > 0 ? sorted[sorted.length - 1] : null;
    const hasT = typeof liveFix.t === "number" && Number.isFinite(liveFix.t);
    const liveT = hasT ? liveFix.t! : (last ? last[0] : 0);

    if (!last) {
      sorted.push([liveT, liveFix.lat, liveFix.lon]);
    } else if (liveT > last[0]) {
      sorted.push([liveT, liveFix.lat, liveFix.lon]);
    } else if (liveT === last[0]) {
      if (Math.abs(last[1] - liveFix.lat) > 1e-7 || Math.abs(last[2] - liveFix.lon) > 1e-7) {
        sorted[sorted.length - 1] = [liveT, liveFix.lat, liveFix.lon];
      }
    } else if (Math.abs(last[1] - liveFix.lat) > 1e-7 || Math.abs(last[2] - liveFix.lon) > 1e-7) {
      sorted.push([last[0], liveFix.lat, liveFix.lon]);
    }
  }

  if (sorted.length === 0) return [];
  const latestT = sorted[sorted.length - 1][0];
  const cutoff = latestT - windowMs;
  return sorted.filter((p) => p[0] >= cutoff);
}

/**
 * Splits projected screen coordinates (oldest -> newest) into consecutive line segments
 * with an ascending alpha ramp from `minAlpha` (oldest) to `maxAlpha` (newest).
 */
export function buildTrailSegments(
  projected: readonly ScreenPoint[],
  minAlpha = 0.22,
  maxAlpha = 0.98,
): TrailSegment[] {
  if (projected.length < 2) return [];
  const segCount = projected.length - 1;
  const out: TrailSegment[] = [];
  for (let i = 0; i < segCount; i++) {
    const t = segCount === 1 ? 1 : i / (segCount - 1);
    const alpha = minAlpha + (maxAlpha - minAlpha) * t;
    out.push({
      x1: projected[i].x,
      y1: projected[i].y,
      x2: projected[i + 1].x,
      y2: projected[i + 1].y,
      alpha,
    });
  }
  return out;
}

export function computeTrailLengthPx(projected: readonly ScreenPoint[]): number {
  let total = 0;
  for (let i = 1; i < projected.length; i++) {
    total += Math.hypot(projected[i].x - projected[i - 1].x, projected[i].y - projected[i - 1].y);
  }
  return total;
}

export function matchTrailForAircraft(
  trails: readonly FlightTrail[],
  ac: { flightId?: string; tail: string },
): FlightTrail | null {
  if (ac.flightId) {
    const byId = trails.find((tr) => tr.flightId && tr.flightId === ac.flightId);
    if (byId) return byId;
  }
  return trails.find((tr) => tr.tail === ac.tail) ?? null;
}

function getSafe2dContext(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  if (typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent)) {
    return null;
  }
  try {
    return canvas.getContext("2d");
  } catch {
    return null;
  }
}

/**
 * Renders live aircraft trails onto a DPR-scaled canvas layer plus a lightweight metadata overlay
 * exposing `data-testid="live-trail"` attributes for inspection and e2e assertions.
 * Uses CSS classes only (no inline style attributes) to keep strict CSP compliance.
 */
export function renderTrailsOverlay(
  container: HTMLElement,
  opts: {
    viewportPx: { w: number; h: number };
    lineWidthPx: number;
    items: readonly RenderTrailItem[];
    project: (lat: number, lon: number) => { left: number; top: number };
  },
): void {
  container
    .querySelectorAll("[data-trails-canvas], [data-trails-layer]")
    .forEach((el) => el.remove());

  const drawable = opts.items.filter(
    (item) => item.trailMinutes !== TRAIL_OFF && item.points.length >= 2,
  );
  if (drawable.length === 0) return;

  const dpr =
    typeof window !== "undefined" && Number.isFinite(window.devicePixelRatio)
      ? Math.max(1, Math.min(3, window.devicePixelRatio))
      : 1;

  const canvas = document.createElement("canvas");
  canvas.className = "tile-map-trails-canvas";
  canvas.setAttribute("data-trails-canvas", "");
  canvas.setAttribute("aria-hidden", "true");
  canvas.width = Math.max(1, Math.round(opts.viewportPx.w * dpr));
  canvas.height = Math.max(1, Math.round(opts.viewportPx.h * dpr));

  const metaLayer = document.createElement("div");
  metaLayer.className = "tile-map-trails-meta";
  metaLayer.setAttribute("data-trails-layer", "");
  metaLayer.setAttribute("aria-hidden", "true");

  const ctx = getSafe2dContext(canvas);
  if (ctx) {
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
  }

  // Draw nearby non-club trails underneath own-club trails so club aircraft stay prominent.
  const ordered = [
    ...drawable.filter((item) => item.nonClub === true),
    ...drawable.filter((item) => item.nonClub !== true),
  ];

  for (const item of ordered) {
    const projected: ScreenPoint[] = item.points.map((pt) => {
      const p = opts.project(pt[1], pt[2]);
      return { x: p.left, y: p.top };
    });
    const isNonClub = item.nonClub === true;
    const segments = isNonClub
      ? buildTrailSegments(projected, 0.14, 0.62)
      : buildTrailSegments(projected);
    if (segments.length === 0) continue;

    if (ctx) {
      const [r, g, b] = item.rgb;
      const strokeWidth = isNonClub
        ? Math.max(1.5, opts.lineWidthPx * 0.65)
        : opts.lineWidthPx;
      ctx.setLineDash(isNonClub ? [6, 5] : []);
      // High-contrast dark casing so trails stay legible over bright or dark map imagery.
      ctx.lineWidth = strokeWidth + (isNonClub ? 2 : 3.5);
      const casingFactor = isNonClub ? 0.45 : 0.78;
      for (const seg of segments) {
        ctx.strokeStyle = `rgba(10, 15, 22, ${(seg.alpha * casingFactor).toFixed(3)})`;
        ctx.beginPath();
        ctx.moveTo(seg.x1, seg.y1);
        ctx.lineTo(seg.x2, seg.y2);
        ctx.stroke();
      }
      // Primary coloured trail fading oldest -> newest.
      ctx.lineWidth = strokeWidth;
      for (const seg of segments) {
        ctx.strokeStyle = `rgba(${r}, ${g}, ${b}, ${seg.alpha.toFixed(3)})`;
        ctx.beginPath();
        ctx.moveTo(seg.x1, seg.y1);
        ctx.lineTo(seg.x2, seg.y2);
        ctx.stroke();
      }
    }

    const spanMs = Math.max(
      0,
      item.points[item.points.length - 1][0] - item.points[0][0],
    );
    const lengthPx = computeTrailLengthPx(projected);

    const meta = document.createElement("div");
    meta.setAttribute("data-testid", "live-trail");
    meta.setAttribute("data-trail-tail", item.tail);
    meta.setAttribute("data-trail-flight-id", item.flightId);
    meta.setAttribute("data-trail-minutes", String(item.trailMinutes));
    meta.setAttribute("data-trail-points", String(item.points.length));
    meta.setAttribute("data-trail-span-ms", String(Math.round(spanMs)));
    meta.setAttribute("data-trail-length-px", String(Math.round(lengthPx)));
    if (isNonClub) {
      meta.setAttribute("data-non-club", "true");
    }
    if (item.tailSource) {
      meta.setAttribute("data-tail-source", item.tailSource);
    }
    metaLayer.appendChild(meta);
  }

  if (ctx) {
    ctx.restore();
  }

  const tilesEl = container.querySelector("[data-tiles]");
  if (tilesEl && tilesEl.parentElement === container) {
    tilesEl.insertAdjacentElement("afterend", canvas);
    canvas.insertAdjacentElement("afterend", metaLayer);
  } else {
    container.prepend(metaLayer);
    container.prepend(canvas);
  }
}
