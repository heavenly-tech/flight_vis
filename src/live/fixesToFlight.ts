import { ellipsoidToMsl, ensureGeoidLoaded, isGeoidReady } from "../geo/geoid";
import { enrichFlight } from "../track/enrich";
import type { Flight } from "../track/types";
import { ALT_REF_WGS84_ELLIPSOID, type LiveFix } from "./types";

const DEG_TO_RAD = Math.PI / 180;

function fixAltitudeMsl(
  fix: LiveFix,
  opts: { ellipsoidReady: boolean },
): { msl: number; ellipsoid?: number; pendingMsl?: boolean } {
  if (fix.alt == null || !Number.isFinite(fix.alt)) {
    return { msl: 0 };
  }
  if (fix.alt_ref === ALT_REF_WGS84_ELLIPSOID) {
    const ellipsoid = fix.alt;
    if (!opts.ellipsoidReady) {
      return { msl: ellipsoid, ellipsoid, pendingMsl: true };
    }
    return { msl: ellipsoidToMsl(fix.lat, fix.lon, ellipsoid), ellipsoid };
  }
  return { msl: fix.alt };
}

export async function fixesToFlight(
  fixes: LiveFix[],
  opts: { name?: string; fileName?: string } = {},
): Promise<Flight> {
  if (fixes.length < 2) {
    throw new Error("At least two fixes are required.");
  }

  const needsEllipsoid = fixes.some((f) => f.alt_ref === ALT_REF_WGS84_ELLIPSOID);
  if (needsEllipsoid) await ensureGeoidLoaded();
  const ellipsoidReady = !needsEllipsoid || isGeoidReady();

  const sorted = [...fixes].sort((a, b) => a.t - b.t);
  const meta = sorted.map((fix) => {
    const { msl, ellipsoid } = fixAltitudeMsl(fix, { ellipsoidReady });
    const speedMps = fix.speed != null && Number.isFinite(fix.speed) ? fix.speed : undefined;
    return {
      fix,
      msl,
      ellipsoid,
      speedMps,
    };
  });

  const raw = meta.map(({ fix, msl, speedMps }) => ({
    lat: fix.lat,
    lon: fix.lon,
    alt: msl,
    time: fix.t,
    speedMps,
  }));

  const name = opts.name ?? "Live track";
  const fileName = opts.fileName ?? `${name}.gpx`;
  const flight = enrichFlight(name, "gpx", fileName, raw);

  for (let i = 0; i < flight.points.length; i += 1) {
    const { fix, ellipsoid } = meta[i];
    if (ellipsoid != null) {
      flight.points[i].positionAlt = ellipsoid;
    }
    if (fix.accuracy != null && Number.isFinite(fix.accuracy)) {
      flight.points[i].accuracyM = fix.accuracy;
    }
    if (fix.course != null && Number.isFinite(fix.course) && i > 0) {
      flight.points[i].headingRad = fix.course * DEG_TO_RAD;
    }
  }

  return flight;
}