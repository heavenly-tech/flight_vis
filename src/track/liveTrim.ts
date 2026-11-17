import { enrichFlight } from "./enrich";
import type { Flight, TrackPoint } from "./types";

export const LIVE_TAIL_PRESETS_MIN = [5, 10, 15, 20, 30] as const;

export type LiveTailMinutes = (typeof LIVE_TAIL_PRESETS_MIN)[number];

/** Keep only the last `tailMinutes` of the track (drop older points at the start). */
export function trimTailEnd(flight: Flight, tailMinutes: number): Flight {
  if (tailMinutes <= 0 || flight.points.length < 2) return flight;

  const endMs = flight.points[flight.points.length - 1].time;
  const cutoff = endMs - tailMinutes * 60 * 1000;
  const kept = flight.points.filter((p) => p.time >= cutoff);
  if (kept.length < 2) return flight;

  const raw = kept.map((p: TrackPoint) => ({
    lat: p.lat,
    lon: p.lon,
    alt: p.alt,
    time: p.time,
    speedMps: p.speedMps,
  }));

  const trimmed = enrichFlight(flight.name, flight.source, flight.fileName, raw);

  for (let i = 0; i < trimmed.points.length; i += 1) {
    const src = kept[i];
    if (src.positionAlt != null) trimmed.points[i].positionAlt = src.positionAlt;
    if (src.accuracyM != null) trimmed.points[i].accuracyM = src.accuracyM;
  }

  return trimmed;
}