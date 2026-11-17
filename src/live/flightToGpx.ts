import type { Flight } from "../track/types";
import { trackPointGpxLines } from "./gpxLive";

function escapeXml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}

export function flightToGpx(flight: Flight): string {
  const pts = flight.points
    .map((p) => {
      const altEllipsoid = p.positionAlt ?? p.alt;
      return trackPointGpxLines({
        lat: p.lat,
        lon: p.lon,
        time: p.time,
        speedMps: p.speedMps,
        altMsl: p.alt,
        altEllipsoid,
        accuracyM: p.accuracyM,
      });
    })
    .join("\n");

  const name = escapeXml(flight.name);

  return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="flight_vis" xmlns="http://www.topografix.com/GPX/1/1" xmlns:fv="http://flight_vis.local/gpx/1">
  <metadata>
    <name>${name}</name>
  </metadata>
  <trk>
    <name>${name}</name>
    <trkseg>
${pts}
    </trkseg>
  </trk>
</gpx>
`;
}