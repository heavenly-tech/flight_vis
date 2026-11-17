import { describe, expect, it } from "vitest";
import { geoidUndulationM } from "../geo/geoid";
import { fixesToFlight } from "./fixesToFlight";
import type { LiveFix } from "./types";

describe("fixesToFlight", () => {
  it("stores ellipsoid positionAlt and MSL alt", () => {
    const lat = -33.45;
    const lon = -70.66;
    const hEllipsoid = 1200;
    const fixes: LiveFix[] = [
      { t: 1000, lat, lon, alt: hEllipsoid, alt_ref: "wgs84_ellipsoid" },
      { t: 2000, lat: lat + 0.01, lon, alt: hEllipsoid + 10, alt_ref: "wgs84_ellipsoid" },
    ];
    const flight = fixesToFlight(fixes);
    const n = geoidUndulationM(lat, lon);
    expect(flight.points[0].positionAlt).toBe(hEllipsoid);
    expect(flight.points[0].alt).toBeCloseTo(hEllipsoid - n, 3);
  });
});