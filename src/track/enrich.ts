import { haversineMeters, headingRadians } from "./geo";
import type { Flight, TrackPoint } from "./types";

type RawFix = {
  lat: number;
  lon: number;
  alt: number;
  time: number;
  speedMps?: number;
};

export function enrichFlight(
  name: string,
  source: Flight["source"],
  fileName: string,
  raw: RawFix[],
): Flight {
  if (raw.length < 2) {
    throw new Error("Track needs at least two points.");
  }

  const sorted = [...raw].sort((a, b) => a.time - b.time);
  const points: TrackPoint[] = [];
  let distanceM = 0;

  for (let i = 0; i < sorted.length; i += 1) {
    const cur = sorted[i];
    const prev = sorted[i - 1] ?? cur;
    const dt = Math.max(0, (cur.time - prev.time) / 1000);
    const step = i === 0 ? 0 : haversineMeters(prev.lat, prev.lon, cur.lat, cur.lon);
    distanceM += step;

    const computedSpeed = dt > 0 ? step / dt : 0;
    const speedMps = cur.speedMps != null && Number.isFinite(cur.speedMps) ? cur.speedMps : computedSpeed;
    const varioMps = dt > 0 ? (cur.alt - prev.alt) / dt : 0;
    const headingRad =
      i === 0
        ? headingRadians(cur.lat, cur.lon, sorted[1].lat, sorted[1].lon)
        : headingRadians(prev.lat, prev.lon, cur.lat, cur.lon);

    points.push({
      lat: cur.lat,
      lon: cur.lon,
      alt: cur.alt,
      time: cur.time,
      speedMps,
      varioMps,
      distanceM,
      headingRad,
    });
  }

  smoothVario(points);

  const alts = points.map((p) => p.alt);
  const speeds = points.map((p) => p.speedMps);
  const varios = points.map((p) => p.varioMps);

  return {
    name,
    source,
    fileName,
    points,
    durationMs: points[points.length - 1].time - points[0].time,
    distanceM,
    minAlt: Math.min(...alts),
    maxAlt: Math.max(...alts),
    maxSpeedMps: Math.max(...speeds),
    minVario: Math.min(...varios),
    maxVario: Math.max(...varios),
  };
}

function smoothVario(points: TrackPoint[], window = 5): void {
  const half = Math.floor(window / 2);
  const raw = points.map((p) => p.varioMps);
  for (let i = 0; i < points.length; i += 1) {
    let sum = 0;
    let n = 0;
    for (let j = i - half; j <= i + half; j += 1) {
      if (j < 0 || j >= raw.length) continue;
      sum += raw[j];
      n += 1;
    }
    points[i].varioMps = n > 0 ? sum / n : 0;
  }
}
