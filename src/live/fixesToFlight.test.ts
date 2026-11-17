import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { geoidUndulationM, installGeoidGrid, resetGeoidForTests, type GeoidMeta } from "../geo/geoid";
import { fixesToFlight } from "./fixesToFlight";
import type { LiveFix } from "./types";

const root = path.resolve(import.meta.dirname, "../..");

beforeAll(() => {
  const meta = JSON.parse(
    readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
  ) as GeoidMeta;
  const bin = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));
  installGeoidGrid(meta, new Float32Array(bin.buffer, bin.byteOffset, bin.byteLength / 4));
});

describe("fixesToFlight", () => {
  it("stores ellipsoid positionAlt and MSL alt", async () => {
    const lat = -33.45;
    const lon = -70.66;
    const hEllipsoid = 1200;
    const fixes: LiveFix[] = [
      { t: 1000, lat, lon, alt: hEllipsoid, alt_ref: "wgs84_ellipsoid" },
      { t: 2000, lat: lat + 0.01, lon, alt: hEllipsoid + 10, alt_ref: "wgs84_ellipsoid" },
    ];
    const flight = await fixesToFlight(fixes);
    const n = geoidUndulationM(lat, lon);
    expect(flight.points[0].positionAlt).toBe(hEllipsoid);
    expect(flight.points[0].alt).toBeCloseTo(hEllipsoid - n, 3);
  });

  it("preserves ellipsoid height in track when geoid is unavailable", async () => {
    resetGeoidForTests();
    const hEllipsoid = 900;
    const fixes: LiveFix[] = [
      { t: 0, lat: -33.4, lon: -70.57, alt: hEllipsoid, alt_ref: "wgs84_ellipsoid" },
      { t: 1000, lat: -33.41, lon: -70.57, alt: hEllipsoid, alt_ref: "wgs84_ellipsoid" },
    ];
    const flight = await fixesToFlight(fixes);
    expect(flight.points[0].positionAlt).toBe(hEllipsoid);
    expect(flight.points[0].alt).toBe(hEllipsoid);
  });
});