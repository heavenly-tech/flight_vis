import { describe, expect, it } from "vitest";
import { ellipsoidToMsl, geoidUndulationM } from "./geoid";

/**
 * Reference undulation N (m) from GeographicLib Geoid EGM96-5 (egm96-5).
 * Tolerance accounts for bilinear interpolation on the NGA EGM96 grid.
 */
const GEOGRAPHICLIB_EGM96_REF = [
  { name: "Santiago", lat: -33.4, lon: -70.57, n: 27.467 },
  { name: "Punta Arenas", lat: -53.163, lon: -70.917, n: 8.851 },
  { name: "Paris", lat: 48.8566, lon: 2.3522, n: 44.563 },
] as const;

/** Max delta allowed vs published GeographicLib EGM96-5 samples (bilinear NGA grid). */
const N_TOLERANCE_M = 0.25;

describe("geoid EGM96 (NGA grid via egm96-universal)", () => {
  it.each(GEOGRAPHICLIB_EGM96_REF)("$name matches GeographicLib EGM96 within $tolerance m", ({ lat, lon, n }) => {
    expect(Math.abs(geoidUndulationM(lat, lon) - n)).toBeLessThanOrEqual(N_TOLERANCE_M);
  });

  it("converts ellipsoid height to MSL", () => {
    const lat = -33.4;
    const lon = -70.57;
    const hEllipsoid = 1030;
    const n = geoidUndulationM(lat, lon);
    expect(ellipsoidToMsl(lat, lon, hEllipsoid)).toBeCloseTo(hEllipsoid - n, 4);
  });
});