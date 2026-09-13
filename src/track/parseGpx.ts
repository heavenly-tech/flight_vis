import { enrichFlight } from "./enrich";
import type { Flight } from "./types";

function textOf(el: Element, tag: string): string | null {
  const node = el.getElementsByTagName(tag)[0];
  return node?.textContent?.trim() || null;
}

export function parseGpx(xml: string, fileName: string): Flight {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  if (doc.querySelector("parsererror")) {
    throw new Error("GPX file is not valid XML.");
  }

  const fixes = [...doc.getElementsByTagName("trkpt"), ...doc.getElementsByTagName("rtept")];
  if (fixes.length === 0) {
    throw new Error("No track points found in GPX.");
  }

  const raw = fixes.flatMap((el) => {
    const lat = Number(el.getAttribute("lat"));
    const lon = Number(el.getAttribute("lon"));
    if (!Number.isFinite(lat) || !Number.isFinite(lon)) return [];
    const ele = Number(textOf(el, "ele") ?? "0");
    const timeText = textOf(el, "time");
    const time = timeText ? Date.parse(timeText) : Number.NaN;
    const speedText = textOf(el, "speed");
    const speedMps = speedText != null ? Number(speedText) : undefined;
    return [
      {
        lat,
        lon,
        alt: Number.isFinite(ele) ? ele : 0,
        time: Number.isFinite(time) ? time : 0,
        speedMps: speedMps != null && Number.isFinite(speedMps) ? speedMps : undefined,
      },
    ];
  });

  if (raw.every((p) => p.time === 0)) {
    const start = Date.now();
    raw.forEach((p, i) => {
      p.time = start + i * 1000;
    });
  }

  const name =
    textOf(doc.documentElement, "desc") ||
    textOf(doc.documentElement, "name") ||
    fileName.replace(/\.[^.]+$/, "");

  return enrichFlight(name, "gpx", fileName, raw);
}
