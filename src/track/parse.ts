import { parseGpx, gpxNeedsGeoid } from "./parseGpx";
import { parseIgc } from "./parseIgc";
import type { Flight, Sample } from "./types";
import { ensureGeoidLoaded, ensureGeoidLoading } from "../geo/geoid";

export function needsGeoidForTrack(text: string, fileName: string): boolean {
  const trimmed = text.trim();
  const lower = fileName.toLowerCase();
  if (lower.endsWith(".igc")) return false;
  if (lower.endsWith(".gpx") || trimmed.startsWith("<") || trimmed.includes("<gpx")) {
    return gpxNeedsGeoid(text);
  }
  return false;
}

export async function parseFlight(text: string, fileName: string): Promise<Flight> {
  const lower = fileName.toLowerCase();
  const trimmed = text.trim();
  if (lower.endsWith(".gpx") || trimmed.startsWith("<") || trimmed.includes("<gpx")) {
    if (gpxNeedsGeoid(text)) {
      ensureGeoidLoading();
      await ensureGeoidLoaded();
    }
    return parseGpx(text, fileName);
  }
  if (lower.endsWith(".igc") || /^[a-z]/i.test(trimmed[0] ?? "")) {
    return parseIgc(text, fileName);
  }
  try {
    if (gpxNeedsGeoid(text)) {
      ensureGeoidLoading();
      await ensureGeoidLoaded();
    }
    return parseGpx(text, fileName);
  } catch {
    return parseIgc(text, fileName);
  }
}

export function sampleAt(flight: Flight, timeMs: number): Sample {
  const pts = flight.points;
  const t = Math.min(Math.max(timeMs, pts[0].time), pts[pts.length - 1].time);
  let lo = 0;
  let hi = pts.length - 1;
  while (lo < hi) {
    const mid = Math.floor((lo + hi) / 2);
    if (pts[mid].time < t) lo = mid + 1;
    else hi = mid;
  }
  const index = Math.max(1, lo);
  const b = pts[index];
  const a = pts[index - 1];
  const span = Math.max(1, b.time - a.time);
  const u = (t - a.time) / span;
  const point: Sample["point"] = {
    lat: lerp(a.lat, b.lat, u),
    lon: lerp(a.lon, b.lon, u),
    alt: lerp(a.alt, b.alt, u),
    time: t,
    speedMps: lerp(a.speedMps, b.speedMps, u),
    varioMps: lerp(a.varioMps, b.varioMps, u),
    distanceM: lerp(a.distanceM, b.distanceM, u),
    headingRad: lerpAngle(a.headingRad, b.headingRad, u),
  };
  if (a.positionAlt != null && b.positionAlt != null) {
    point.positionAlt = lerp(a.positionAlt, b.positionAlt, u);
  } else if (a.positionAlt != null || b.positionAlt != null) {
    point.positionAlt = a.positionAlt ?? b.positionAlt;
  }
  if (a.accuracyM != null && b.accuracyM != null) {
    point.accuracyM = lerp(a.accuracyM, b.accuracyM, u);
  } else if (a.accuracyM != null || b.accuracyM != null) {
    point.accuracyM = a.accuracyM ?? b.accuracyM;
  }
  return { index: index - 1, t, point };
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

function lerpAngle(a: number, b: number, t: number): number {
  let diff = b - a;
  while (diff > Math.PI) diff -= Math.PI * 2;
  while (diff < -Math.PI) diff += Math.PI * 2;
  return a + diff * t;
}