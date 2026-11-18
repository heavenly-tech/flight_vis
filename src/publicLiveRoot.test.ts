import { describe, expect, it } from "vitest";
import { DEMO_FLIGHT_SLUG } from "./demoFlight";
import {
  demoFallbackPath,
  isFullGlobeEntry,
  isSiteRootPath,
  publicLiveSummaryUrl,
  resolvePublicRootRedirect,
} from "./publicLiveRoot";

describe("publicLiveRoot", () => {
  it("treats /map and ?all=1 as full globe", () => {
    expect(isFullGlobeEntry("/map", "")).toBe(true);
    expect(isFullGlobeEntry("/map/", "")).toBe(true);
    expect(isFullGlobeEntry("/", "?all=1")).toBe(true);
    expect(isFullGlobeEntry("/", "")).toBe(false);
  });

  it("scopes summary URL with club", () => {
    expect(publicLiveSummaryUrl(null)).toBe("/api/public/live-summary");
    expect(publicLiveSummaryUrl("demo")).toBe("/api/public/live-summary?club=demo");
  });

  it("resolves 0, 1 and N live public flights", () => {
    expect(resolvePublicRootRedirect({ flights: [] }, null)).toBeNull();
    expect(resolvePublicRootRedirect({ flights: [{ flightId: "f1", tail: "N1" }] }, null)).toBe(
      "/club-flight.html?id=f1",
    );
    expect(
      resolvePublicRootRedirect(
        {
          flights: [
            { flightId: "f1", tail: "N1" },
            { flightId: "f2", tail: "N2" },
          ],
        },
        "my-club",
      ),
    ).toBe("/public-map.html?club=my-club");
  });

  it("uses demo constant for empty fallback path", () => {
    expect(demoFallbackPath()).toBe(`/${DEMO_FLIGHT_SLUG}`);
    expect(isSiteRootPath("/")).toBe(true);
    expect(isSiteRootPath("/index.html")).toBe(true);
    expect(isSiteRootPath("/demo")).toBe(false);
  });
});