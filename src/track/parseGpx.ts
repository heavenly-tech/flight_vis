import { ellipsoidToMsl, isGeoidReady } from "../geo/geoid";
import { enrichFlight } from "./enrich";
import type { Flight } from "./types";
import { ALT_REF_WGS84_ELLIPSOID } from "../live/types";

export function gpxNeedsGeoid(xml: string): boolean {
  return /wgs84_ellipsoid/i.test(xml) || /fv:altRef/i.test(xml);
}

const FV_NS = "http://flight_vis.local/gpx/1";

function textOf(el: Element, tag: string): string | null {
  const node = el.getElementsByTagName(tag)[0];
  return node?.textContent?.trim() || null;
}

function fvText(el: Element, tag: string): string | null {
  const node = el.getElementsByTagNameNS(FV_NS, tag)[0] ?? el.getElementsByTagName(`fv:${tag}`)[0];
  return node?.textContent?.trim() ?? null;
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
    const mslText = fvText(el, "msl");
    const altRef = fvText(el, "altRef");
    const accText = fvText(el, "accuracy");
    const timeText = textOf(el, "time");
    const time = timeText ? Date.parse(timeText) : Number.NaN;
    const speedText = textOf(el, "speed");
    const speedMps = speedText != null ? Number(speedText) : undefined;
    let altMsl = Number.isFinite(ele) ? ele : 0;
    let positionAlt: number | undefined;
    if (altRef === ALT_REF_WGS84_ELLIPSOID || mslText != null) {
      positionAlt = Number.isFinite(ele) ? ele : undefined;
      if (mslText != null && Number.isFinite(Number(mslText))) {
        altMsl = Number(mslText);
      } else if (positionAlt != null) {
        altMsl = isGeoidReady() ? ellipsoidToMsl(lat, lon, positionAlt) : positionAlt;
      }
    }
    const accuracyM = accText != null && Number.isFinite(Number(accText)) ? Number(accText) : undefined;
    return [
      {
        lat,
        lon,
        alt: altMsl,
        positionAlt,
        accuracyM,
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

  const flight = enrichFlight(name, "gpx", fileName, raw);
  for (let i = 0; i < flight.points.length; i += 1) {
    if (raw[i].positionAlt != null) flight.points[i].positionAlt = raw[i].positionAlt;
    if (raw[i].accuracyM != null) flight.points[i].accuracyM = raw[i].accuracyM;
  }
  return flight;
}