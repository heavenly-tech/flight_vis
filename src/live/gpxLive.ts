import { ALT_REF_WGS84_ELLIPSOID, type LiveFix } from "./types";

const FV_NS = "http://flight_vis.local/gpx/1";

function localText(el: Element, tag: string): string | null {
  const node = el.getElementsByTagNameNS(FV_NS, tag)[0] ?? el.getElementsByTagName(`fv:${tag}`)[0];
  return node?.textContent?.trim() ?? null;
}

export function trackPointGpxLines(p: {
  lat: number;
  lon: number;
  time: number;
  speedMps: number;
  altMsl: number;
  altEllipsoid: number;
  accuracyM?: number;
}): string {
  const speed = Number.isFinite(p.speedMps) ? `\n        <speed>${p.speedMps.toFixed(3)}</speed>` : "";
  const acc =
    p.accuracyM != null && Number.isFinite(p.accuracyM)
      ? `\n        <fv:accuracy>${p.accuracyM.toFixed(1)}</fv:accuracy>`
      : "";
  return `      <trkpt lat="${p.lat}" lon="${p.lon}">
        <ele>${p.altEllipsoid.toFixed(2)}</ele>
        <fv:msl>${p.altMsl.toFixed(2)}</fv:msl>
        <fv:altRef>${ALT_REF_WGS84_ELLIPSOID}</fv:altRef>${acc}
        <time>${new Date(p.time).toISOString()}</time>${speed}
      </trkpt>`;
}

export function liveFixesFromGpx(xml: string): LiveFix[] {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  const pts = [...doc.getElementsByTagName("trkpt"), ...doc.getElementsByTagName("rtept")];
  const fixes: LiveFix[] = [];
  for (const el of pts) {
    const lat = Number(el.getAttribute("lat"));
    const lon = Number(el.getAttribute("lon"));
    if (!Number.isFinite(lat) || !Number.isFinite(lon)) continue;
    const timeText = el.getElementsByTagName("time")[0]?.textContent?.trim();
    const t = timeText ? Date.parse(timeText) : Number.NaN;
    if (!Number.isFinite(t)) continue;
    const ele = Number(el.getElementsByTagName("ele")[0]?.textContent ?? "0");
    const mslText = localText(el, "msl");
    const altRef = localText(el, "altRef");
    const speedText = el.getElementsByTagName("speed")[0]?.textContent?.trim();
    const accText = localText(el, "accuracy");
    const fix: LiveFix = { t, lat, lon };
    if (altRef === ALT_REF_WGS84_ELLIPSOID || mslText != null) {
      if (Number.isFinite(ele)) {
        fix.alt = ele;
        fix.alt_ref = ALT_REF_WGS84_ELLIPSOID;
      }
    } else if (Number.isFinite(ele)) {
      fix.alt = ele;
    }
    if (speedText) {
      const speed = Number(speedText);
      if (Number.isFinite(speed)) fix.speed = speed;
    }
    if (accText) {
      const accuracy = Number(accText);
      if (Number.isFinite(accuracy)) fix.accuracy = accuracy;
    }
    fixes.push(fix);
  }
  return fixes.sort((a, b) => a.t - b.t);
}