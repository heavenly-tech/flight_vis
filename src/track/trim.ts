import type { Flight, TrackPoint, TrimRange } from "./types";

const MIN_AIR_MPS = 20;
const SUSTAIN_POINTS = 8;
const PAD_MS = 12_000;

export function fullRange(flight: Flight): TrimRange {
  return { startMs: flight.points[0].time, endMs: flight.points[flight.points.length - 1].time };
}

export function clampTrim(flight: Flight, trim: TrimRange): TrimRange {
  const full = fullRange(flight);
  let startMs = Math.min(Math.max(trim.startMs, full.startMs), full.endMs);
  let endMs = Math.min(Math.max(trim.endMs, full.startMs), full.endMs);
  if (endMs - startMs < 5_000) {
    endMs = Math.min(full.endMs, startMs + 5_000);
    if (endMs - startMs < 5_000) startMs = Math.max(full.startMs, endMs - 5_000);
  }
  return { startMs, endMs };
}

export function suggestTrim(flight: Flight): TrimRange {
  const pts = flight.points;
  const full = fullRange(flight);
  const threshold = Math.max(MIN_AIR_MPS, flight.maxSpeedMps * 0.28);
  const flying = (p: TrackPoint) => p.speedMps >= threshold;

  let first = -1;
  for (let i = 0; i <= pts.length - SUSTAIN_POINTS; i += 1) {
    if (pts.slice(i, i + SUSTAIN_POINTS).every(flying)) {
      first = i;
      break;
    }
  }
  if (first < 0) return full;

  let last = first;
  for (let i = pts.length - SUSTAIN_POINTS; i >= first; i -= 1) {
    if (pts.slice(i, i + SUSTAIN_POINTS).every(flying)) {
      last = i + SUSTAIN_POINTS - 1;
      break;
    }
  }

  return clampTrim(flight, {
    startMs: pts[first].time - PAD_MS,
    endMs: pts[last].time + PAD_MS,
  });
}

export function pointsInTrim(flight: Flight, trim: TrimRange): TrackPoint[] {
  return flight.points.filter((p) => p.time >= trim.startMs && p.time <= trim.endMs);
}

export function rangeStats(flight: Flight, trim: TrimRange): {
  distanceM: number;
  durationMs: number;
  maxAlt: number;
  points: number;
} {
  const pts = pointsInTrim(flight, trim);
  if (pts.length === 0) {
    return { distanceM: 0, durationMs: 0, maxAlt: flight.maxAlt, points: 0 };
  }
  return {
    distanceM: pts[pts.length - 1].distanceM - pts[0].distanceM,
    durationMs: pts[pts.length - 1].time - pts[0].time,
    maxAlt: Math.max(...pts.map((p) => p.alt)),
    points: pts.length,
  };
}

export function newTagId(): string {
  return `tag_${Math.random().toString(36).slice(2, 10)}`;
}
