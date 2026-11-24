import { getLocale, localeBcp47 } from "../i18n/index";

export const CLUB_TIME_ZONE = "America/Santiago";

function getTimeZoneOffsetMs(date: Date, timeZone: string): number {
  const dtf = new Intl.DateTimeFormat("en-US", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  });
  const parts = dtf.formatToParts(date);
  const part = (type: Intl.DateTimeFormatPartTypes): number =>
    Number(parts.find((p) => p.type === type)?.value ?? "0");
  const hour = part("hour");
  const asUtc = Date.UTC(
    part("year"),
    part("month") - 1,
    part("day"),
    hour === 24 ? 0 : hour,
    part("minute"),
    part("second"),
  );
  const utcSec = Math.floor(date.getTime() / 1000) * 1000;
  return asUtc - utcSec;
}

/**
 * Parse a YYYY-MM-DD calendar date as end-of-day (23:59:59.999) in America/Santiago.
 * Returns epoch milliseconds (UTC), or undefined for empty / invalid dates.
 */
export function parseSantiagoDateToExpiryMs(dateStr: string): number | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dateStr.trim());
  if (!m) return undefined;
  const year = Number(m[1]);
  const month = Number(m[2]);
  const day = Number(m[3]);
  const probe = new Date(Date.UTC(year, month - 1, day));
  if (
    probe.getUTCFullYear() !== year ||
    probe.getUTCMonth() !== month - 1 ||
    probe.getUTCDate() !== day
  ) {
    return undefined;
  }
  const targetWallUtc = Date.UTC(year, month - 1, day, 23, 59, 59, 0);
  let utcMs = targetWallUtc - getTimeZoneOffsetMs(new Date(targetWallUtc), CLUB_TIME_ZONE);
  utcMs = targetWallUtc - getTimeZoneOffsetMs(new Date(utcMs), CLUB_TIME_ZONE);
  return utcMs + 999;
}

/**
 * Format an epoch millisecond timestamp as a calendar date in America/Santiago.
 * Uses dd/MM/yyyy for Spanish (es-CL) and MM/dd/yyyy for English.
 */
export function formatSantiagoDate(ms: number, locale = getLocale()): string {
  const bcp = localeBcp47(locale);
  const parts = new Intl.DateTimeFormat(bcp, {
    timeZone: CLUB_TIME_ZONE,
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  }).formatToParts(new Date(ms));
  const day = parts.find((p) => p.type === "day")?.value ?? "";
  const month = parts.find((p) => p.type === "month")?.value ?? "";
  const year = parts.find((p) => p.type === "year")?.value ?? "";
  if (locale.toLowerCase().startsWith("en")) {
    return `${month}/${day}/${year}`;
  }
  return `${day}/${month}/${year}`;
}
