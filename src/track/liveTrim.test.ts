import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { installGeoidGrid, type GeoidMeta } from "../geo/geoid";
import { fixesToFlight } from "../live/fixesToFlight";
import type { LiveFix } from "../live/types";
import { trimTailEnd } from "./liveTrim";

const root = path.resolve(import.meta.dirname, "../..");

beforeAll(() => {
  const meta = JSON.parse(
    readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
  ) as GeoidMeta;
  const bin = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));
  installGeoidGrid(meta, new Float32Array(bin.buffer, bin.byteOffset, bin.byteLength / 4));
});

function lineFixes(startMs: number, count: number, stepMs: number): LiveFix[] {
  return Array.from({ length: count }, (_, i) => ({
    t: startMs + i * stepMs,
    lat: -33.4 + i * 0.0001,
    lon: -70.6,
    alt: 1000 + i,
    alt_ref: "wgs84_ellipsoid",
  }));
}

describe("trimTailEnd", () => {
  it("keeps only the last N minutes of fixes", async () => {
    const base = 1_700_000_000_000;
    const flight = await fixesToFlight(lineFixes(base, 20, 60_000), { name: "Live" });
    const trimmed = trimTailEnd(flight, 5);
    const fullDuration = flight.points[flight.points.length - 1].time - flight.points[0].time;
    const tailDuration = trimmed.points[trimmed.points.length - 1].time - trimmed.points[0].time;
    expect(fullDuration).toBe(19 * 60_000);
    expect(tailDuration).toBeLessThanOrEqual(5 * 60_000 + 1);
    expect(tailDuration).toBeGreaterThanOrEqual(4 * 60_000);
    expect(trimmed.points.length).toBeLessThan(flight.points.length);
  });

  it("exposes tail presets for 5–30 minutes", async () => {
    const base = 1_700_000_000_000;
    const flight = await fixesToFlight(lineFixes(base, 40, 60_000), { name: "Live" });
    for (const minutes of [5, 10, 15, 20, 30] as const) {
      const trimmed = trimTailEnd(flight, minutes);
      const duration = trimmed.points[trimmed.points.length - 1].time - trimmed.points[0].time;
      expect(duration).toBeLessThanOrEqual(minutes * 60_000 + 1);
    }
  });
});