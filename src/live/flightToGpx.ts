import type { Flight } from "../track/types";

function isoTime(ms: number): string {
  return new Date(ms).toISOString();
}

export function flightToGpx(flight: Flight): string {
  const pts = flight.points
    .map((p) => {
      const speed = Number.isFinite(p.speedMps) ? `\n        <speed>${p.speedMps.toFixed(3)}</speed>` : "";
      return `      <trkpt lat="${p.lat}" lon="${p.lon}">
        <ele>${p.alt.toFixed(2)}</ele>
        <time>${isoTime(p.time)}</time>${speed}
      </trkpt>`;
    })
    .join("\n");

  const name = escapeXml(flight.name);

  return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="flight_vis" xmlns="http://www.topografix.com/GPX/1/1">
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

function escapeXml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}