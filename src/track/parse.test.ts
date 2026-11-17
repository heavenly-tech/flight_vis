import { describe, expect, it } from "vitest";
import { sampleAt } from "./parse";
import type { Flight } from "./types";

function flight(points: Flight["points"]): Flight {
  const alts = points.map((p) => p.alt);
  const speeds = points.map((p) => p.speedMps);
  const varios = points.map((p) => p.varioMps);
  return {
    name: "t",
    source: "gpx",
    fileName: "t.gpx",
    points,
    durationMs: points[points.length - 1].time - points[0].time,
    distanceM: points[points.length - 1].distanceM,
    minAlt: Math.min(...alts),
    maxAlt: Math.max(...alts),
    maxSpeedMps: Math.max(...speeds),
    minVario: Math.min(...varios),
    maxVario: Math.max(...varios),
  };
}

describe("sampleAt", () => {
  it("preserves positionAlt and accuracyM", () => {
    const f = flight([
      {
        lat: 0,
        lon: 0,
        alt: 100,
        positionAlt: 130,
        accuracyM: 40,
        time: 0,
        speedMps: 10,
        varioMps: 0,
        distanceM: 0,
        headingRad: 0,
      },
      {
        lat: 0,
        lon: 1,
        alt: 110,
        positionAlt: 140,
        accuracyM: 60,
        time: 1000,
        speedMps: 12,
        varioMps: 0,
        distanceM: 100,
        headingRad: 0,
      },
    ]);
    const s = sampleAt(f, 500);
    expect(s.point.positionAlt).toBeCloseTo(135, 5);
    expect(s.point.accuracyM).toBeCloseTo(50, 5);
    expect(s.point.alt).toBeCloseTo(105, 5);
  });
});