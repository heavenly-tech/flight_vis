import { describe, expect, it } from "vitest";
import { ellipsoidToMsl, geoidUndulationM } from "./geoid";

describe("geoid", () => {
  it("returns ~+30 m undulation near Santiago", () => {
    const lat = -33.45;
    const lon = -70.66;
    const n = geoidUndulationM(lat, lon);
    expect(n).toBeGreaterThan(27);
    expect(n).toBeLessThan(32);
  });

  it("converts ellipsoid height to MSL", () => {
    const lat = -33.45;
    const lon = -70.66;
    const hEllipsoid = 1030;
    const n = geoidUndulationM(lat, lon);
    expect(ellipsoidToMsl(lat, lon, hEllipsoid)).toBeCloseTo(hEllipsoid - n, 5);
  });
});