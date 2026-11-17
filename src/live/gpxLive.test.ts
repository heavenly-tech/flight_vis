import { describe, expect, it } from "vitest";
import { fixesToFlight } from "./fixesToFlight";
import { flightToGpx } from "./flightToGpx";
import { liveFixesFromGpx } from "./gpxLive";
import { parseGpx } from "../track/parseGpx";
import { ALT_REF_WGS84_ELLIPSOID, type LiveFix } from "./types";

const fixes: LiveFix[] = [
  {
    t: 1_700_000_000_000,
    lat: -33.4,
    lon: -70.57,
    alt: 1027.5,
    alt_ref: ALT_REF_WGS84_ELLIPSOID,
    accuracy: 12,
    speed: 10,
  },
  {
    t: 1_700_000_001_000,
    lat: -33.41,
    lon: -70.58,
    alt: 1030,
    alt_ref: ALT_REF_WGS84_ELLIPSOID,
    accuracy: 8,
    speed: 11,
  },
];

describe("live GPX ellipsoid round-trip", () => {
  it("keeps WGS84 ellipsoid height for replay", () => {
    const flight = fixesToFlight(fixes, { name: "t", fileName: "t.gpx" });
    const gpx = flightToGpx(flight);
    expect(gpx).toContain("<fv:msl>");
    expect(gpx).toContain("wgs84_ellipsoid");
    const replay = parseGpx(gpx, "t.gpx");
    expect(replay.points[0].positionAlt).toBeCloseTo(1027.5, 1);
    expect(replay.points[0].alt).toBeLessThan(replay.points[0].positionAlt!);
    const round = liveFixesFromGpx(gpx);
    expect(round[0].alt_ref).toBe(ALT_REF_WGS84_ELLIPSOID);
    expect(round[0].alt).toBeCloseTo(1027.5, 1);
  });
});