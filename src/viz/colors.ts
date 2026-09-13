import { Color } from "cesium";
import type { ColorMode, Flight, TrackPoint } from "../track/types";

const STOPS: Array<[number, [number, number, number]]> = [
  [0, [59, 130, 246]],
  [0.22, [34, 211, 238]],
  [0.4, [163, 230, 53]],
  [0.58, [250, 204, 21]],
  [0.78, [249, 115, 22]],
  [1, [239, 68, 68]],
];

export function colorForUnit(t: number): Color {
  const u = Math.min(1, Math.max(0, t));
  let i = 1;
  while (i < STOPS.length && STOPS[i][0] < u) i += 1;
  const [t1, c1] = STOPS[i - 1];
  const [t2, c2] = STOPS[i];
  const f = (u - t1) / Math.max(1e-6, t2 - t1);
  return Color.fromBytes(
    Math.round(c1[0] + (c2[0] - c1[0]) * f),
    Math.round(c1[1] + (c2[1] - c1[1]) * f),
    Math.round(c1[2] + (c2[2] - c1[2]) * f),
    255,
  );
}

export function cssForUnit(t: number): string {
  const c = colorForUnit(t);
  return `rgb(${Math.round(c.red * 255)}, ${Math.round(c.green * 255)}, ${Math.round(c.blue * 255)})`;
}

export function valueForMode(point: TrackPoint, mode: ColorMode): number {
  if (mode === "speed") return point.speedMps;
  if (mode === "vario") return point.varioMps;
  return point.alt;
}

export function normalizeValue(flight: Flight, mode: ColorMode, value: number): number {
  if (mode === "speed") {
    return flight.maxSpeedMps > 0 ? value / flight.maxSpeedMps : 0;
  }
  if (mode === "vario") {
    const mag = Math.max(2, Math.abs(flight.minVario), Math.abs(flight.maxVario));
    return (value + mag) / (2 * mag);
  }
  const span = Math.max(1, flight.maxAlt - flight.minAlt);
  return (value - flight.minAlt) / span;
}
