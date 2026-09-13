import { enrichFlight } from "./enrich";
import type { Flight } from "./types";

function parseLat(token: string): number {
  const deg = Number(token.slice(0, 2));
  const min = Number(token.slice(2, 7)) / 1000;
  const hemi = token.slice(7, 8);
  const value = deg + min / 60;
  return hemi === "S" ? -value : value;
}

function parseLon(token: string): number {
  const deg = Number(token.slice(0, 3));
  const min = Number(token.slice(3, 8)) / 1000;
  const hemi = token.slice(8, 9);
  const value = deg + min / 60;
  return hemi === "W" ? -value : value;
}

function parseIgcDate(lines: string[]): Date {
  for (const line of lines) {
    const dateHeader = line.match(/^HFDTEDATE:(\d{2})(\d{2})(\d{2})/);
    if (dateHeader) {
      const [, dd, mm, yy] = dateHeader;
      return ymd(dd, mm, yy);
    }
    const legacy = line.match(/^HFDTE(\d{2})(\d{2})(\d{2})/);
    if (legacy) {
      const [, dd, mm, yy] = legacy;
      return ymd(dd, mm, yy);
    }
  }
  return new Date();
}

function ymd(dd: string, mm: string, yy: string): Date {
  const year = Number(yy) + (Number(yy) >= 80 ? 1900 : 2000);
  return new Date(Date.UTC(year, Number(mm) - 1, Number(dd)));
}

function headerValue(lines: string[], prefix: string): string | null {
  const line = lines.find((l) => l.startsWith(prefix));
  if (!line) return null;
  const idx = line.indexOf(":");
  return (idx >= 0 ? line.slice(idx + 1) : line.slice(prefix.length)).trim() || null;
}

export function parseIgc(text: string, fileName: string): Flight {
  const lines = text.replace(/\r/g, "").split("\n").map((l) => l.trim()).filter(Boolean);
  const date = parseIgcDate(lines);
  let dayOffset = 0;
  let lastSeconds = -1;

  const raw = lines.flatMap((line) => {
    if (!line.startsWith("B") || line.length < 35) return [];
    const hh = Number(line.slice(1, 3));
    const mi = Number(line.slice(3, 5));
    const ss = Number(line.slice(5, 7));
    if (![hh, mi, ss].every(Number.isFinite)) return [];

    const seconds = hh * 3600 + mi * 60 + ss;
    if (lastSeconds >= 0 && seconds + 60 < lastSeconds) dayOffset += 1;
    lastSeconds = seconds;

    const lat = parseLat(line.slice(7, 15));
    const lon = parseLon(line.slice(15, 24));
    const gpsAlt = Number(line.slice(30, 35));
    const pressureAlt = Number(line.slice(25, 30));
    const alt = Number.isFinite(gpsAlt) && gpsAlt !== 0 ? gpsAlt : pressureAlt;
    if (!Number.isFinite(lat) || !Number.isFinite(lon) || !Number.isFinite(alt)) return [];

    const time = Date.UTC(
      date.getUTCFullYear(),
      date.getUTCMonth(),
      date.getUTCDate() + dayOffset,
      hh,
      mi,
      ss,
    );

    return [{ lat, lon, alt, time }];
  });

  if (raw.length < 2) {
    throw new Error("No B-record fixes found in IGC.");
  }

  const name =
    headerValue(lines, "HFSITSite") ||
    headerValue(lines, "HFGTYGLIDERTYPE") ||
    headerValue(lines, "HFPLTPILOTINCHARGE") ||
    fileName.replace(/\.[^.]+$/, "");

  return enrichFlight(name, "igc", fileName, raw);
}
