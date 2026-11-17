import { readFileSync } from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { installGeoidGrid, type GeoidMeta } from "../geo/geoid";
import { fixesToFlight } from "./fixesToFlight";
import { flightToGpx } from "./flightToGpx";
import { liveFixesFromGpx } from "./gpxLive";
import { parseGpx } from "../track/parseGpx";
import { ALT_REF_WGS84_ELLIPSOID, type LiveFix } from "./types";

const root = path.resolve(import.meta.dirname, "../..");

beforeAll(() => {
  const meta = JSON.parse(
    readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
  ) as GeoidMeta;
  const bin = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));
  installGeoidGrid(meta, new Float32Array(bin.buffer, bin.byteOffset, bin.byteLength / 4));
});

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
  it("keeps WGS84 ellipsoid height for replay", async () => {
    const flight = await fixesToFlight(fixes, { name: "t", fileName: "t.gpx" });
    const gpx = flightToGpx(flight);
    expect(gpx).toContain("<fv:msl>");
    expect(gpx).toContain("wgs84_ellipsoid");
    const replay = await parseGpx(gpx, "t.gpx");
    expect(replay.points[0].positionAlt).toBeCloseTo(1027.5, 1);
    expect(replay.points[0].alt).toBeLessThan(replay.points[0].positionAlt!);
    const round = liveFixesFromGpx(gpx);
    expect(round[0].alt_ref).toBe(ALT_REF_WGS84_ELLIPSOID);
    expect(round[0].alt).toBeCloseTo(1027.5, 1);
  });
});