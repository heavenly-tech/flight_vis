import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { ellipsoidToMsl, geoidUndulationM, installGeoidGrid, type GeoidMeta } from "./geoid";

/**
 * Reference undulation N (m) from GeographicLib Geoid EGM96-5 (egm96-5).
 */
const GEOGRAPHICLIB_EGM96_REF = [
  { name: "Santiago", lat: -33.4, lon: -70.57, n: 27.467 },
  { name: "Punta Arenas", lat: -53.163, lon: -70.917, n: 8.851 },
  { name: "Paris", lat: 48.8566, lon: 2.3522, n: 44.563 },
] as const;

/** Max delta allowed vs published GeographicLib EGM96-5 samples (0.25° grid). */
const N_TOLERANCE_M = 0.25;

const root = path.resolve(import.meta.dirname, "../..");

beforeAll(() => {
  const meta = JSON.parse(
    readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
  ) as GeoidMeta;
  const bin = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));
  installGeoidGrid(meta, new Float32Array(bin.buffer, bin.byteOffset, bin.byteLength / 4));
});

describe("geoid EGM96 (lazy-loaded 0.25° NGA grid)", () => {
  it.each(GEOGRAPHICLIB_EGM96_REF)(
    "$name matches GeographicLib EGM96 within tolerance",
    ({ lat, lon, n }) => {
      expect(Math.abs(geoidUndulationM(lat, lon) - n)).toBeLessThanOrEqual(N_TOLERANCE_M);
    },
  );

  it("converts ellipsoid height to MSL", () => {
    const lat = -33.4;
    const lon = -70.57;
    const hEllipsoid = 1030;
    const n = geoidUndulationM(lat, lon);
    expect(ellipsoidToMsl(lat, lon, hEllipsoid)).toBeCloseTo(hEllipsoid - n, 4);
  });
});