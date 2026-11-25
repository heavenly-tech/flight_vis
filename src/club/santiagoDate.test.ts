import { describe, expect, it } from "vitest";
import { formatSantiagoDate, parseSantiagoDateToExpiryMs } from "./santiagoDate";

describe("santiagoDate", () => {
  it("parses and formats summer (CLST UTC-3) and winter (CLT UTC-4) dates in America/Santiago", () => {
    const summerMs = parseSantiagoDateToExpiryMs("2026-12-31");
    expect(summerMs).toBeDefined();
    // 2026-12-31 23:59:59.999 America/Santiago (UTC-3) = 2027-01-01T02:59:59.999Z
    expect(summerMs).toBe(Date.UTC(2027, 0, 1, 2, 59, 59, 999));
    expect(formatSantiagoDate(summerMs!, "es")).toBe("31/12/2026");
    expect(formatSantiagoDate(summerMs!, "en")).toBe("12/31/2026");

    const winterMs = parseSantiagoDateToExpiryMs("2026-07-15");
    expect(winterMs).toBeDefined();
    // 2026-07-15 23:59:59.999 America/Santiago (UTC-4) = 2026-07-16T03:59:59.999Z
    expect(winterMs).toBe(Date.UTC(2026, 6, 16, 3, 59, 59, 999));
    expect(formatSantiagoDate(winterMs!, "es")).toBe("15/07/2026");
  });

  it("displays UTC timestamps after midnight using America/Santiago local date", () => {
    // 2026-12-31T01:30:00Z is 2026-12-30 22:30:00 in America/Santiago (UTC-3)
    const utcAfterMidnight = Date.UTC(2026, 11, 31, 1, 30, 0);
    expect(formatSantiagoDate(utcAfterMidnight, "es")).toBe("30/12/2026");
  });

  it("handles Chilean DST transition dates consistently", () => {
    const autumnTransition = parseSantiagoDateToExpiryMs("2026-04-04");
    expect(autumnTransition).toBeDefined();
    expect(formatSantiagoDate(autumnTransition!, "es")).toBe("04/04/2026");

    const springTransition = parseSantiagoDateToExpiryMs("2026-09-05");
    expect(springTransition).toBeDefined();
    expect(formatSantiagoDate(springTransition!, "es")).toBe("05/09/2026");
  });

  it("rejects empty or invalid calendar dates", () => {
    expect(parseSantiagoDateToExpiryMs("")).toBeUndefined();
    expect(parseSantiagoDateToExpiryMs("2026-02-30")).toBeUndefined();
    expect(parseSantiagoDateToExpiryMs("not-a-date")).toBeUndefined();
  });
});
