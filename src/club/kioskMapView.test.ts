import { describe, expect, it } from "vitest";
import {
  computeKioskView,
  DEFAULT_BASE_VIEW_KM,
  getFallbackBase,
  getFallbackViewKm,
  latToTileY,
  lonToTileX,
  metersPerPixel,
} from "./kioskMapView";

const SANTIAGO = { lat: -33.4489, lon: -70.6693 };
const TOBALABA = { lat: -33.456, lon: -70.548 };

const VIEWPORT = { w: 1920, h: 1080 };

describe("kioskMapView", () => {
  describe("computeKioskView", () => {
    it("no flights with base returns base center", () => {
      const result = computeKioskView(TOBALABA, [], VIEWPORT);
      expect(result.centerLat).toBeCloseTo(TOBALABA.lat, 5);
      expect(result.centerLon).toBeCloseTo(TOBALABA.lon, 5);
      expect(result.viewType).toBe("base");
    });

    it("no base and no flights returns fallback", () => {
      const result = computeKioskView(null, [], VIEWPORT);
      const fallback = getFallbackBase();
      expect(result.centerLat).toBeCloseTo(fallback.lat, 5);
      expect(result.centerLon).toBeCloseTo(fallback.lon, 5);
      expect(result.viewType).toBe("fallback");
    });

    it("one flight with base includes both in bbox", () => {
      const aircraft = [{ lat: -33.5, lon: -70.6 }];
      const result = computeKioskView(TOBALABA, aircraft, VIEWPORT);
      
      // Center should be between base and aircraft
      expect(result.centerLat).toBeGreaterThanOrEqual(Math.min(TOBALABA.lat, -33.5));
      expect(result.centerLat).toBeLessThanOrEqual(Math.max(TOBALABA.lat, -33.5));
      expect(result.centerLon).toBeGreaterThanOrEqual(Math.min(TOBALABA.lon, -70.6));
      expect(result.centerLon).toBeLessThanOrEqual(Math.max(TOBALABA.lon, -70.6));
      expect(result.viewType).toBe("flights");
    });

    it("several flights with base includes all in bbox", () => {
      const aircraft = [
        { lat: -33.5, lon: -70.6 },
        { lat: -33.4, lon: -70.7 },
        { lat: -33.6, lon: -70.5 },
      ];
      const result = computeKioskView(TOBALABA, aircraft, VIEWPORT);
      
      // All points should be within the bbox
      expect(result.viewType).toBe("flights");
      expect(result.zoom).toBeGreaterThanOrEqual(3);
      expect(result.zoom).toBeLessThanOrEqual(15);
    });

    it("zoom is clamped to min", () => {
      // Very far apart points should still be clamped
      const aircraft = [
        { lat: -33.5, lon: -70.6 },
        { lat: 33.5, lon: 70.6 }, // Opposite side of world
      ];
      const result = computeKioskView(TOBALABA, aircraft, VIEWPORT);
      expect(result.zoom).toBe(3);
    });

    it("zoom is clamped to max", () => {
      // Very close points should still be clamped
      const aircraft = [
        { lat: -33.456, lon: -70.548 },
        { lat: -33.456001, lon: -70.548001 }, // Very close
      ];
      const result = computeKioskView(TOBALABA, aircraft, VIEWPORT);
      expect(result.zoom).toBe(15);
    });

    it("when flights go back to zero the view equals the base view", () => {
      const result1 = computeKioskView(TOBALABA, [], VIEWPORT);
      const result2 = computeKioskView(TOBALABA, [{ lat: -33.5, lon: -70.6 }], VIEWPORT);
      const result3 = computeKioskView(TOBALABA, [], VIEWPORT);
      
      // After having flights and going back to zero, should return to base view
      expect(result1.centerLat).toBeCloseTo(result3.centerLat, 5);
      expect(result1.centerLon).toBeCloseTo(result3.centerLon, 5);
      expect(result1.viewType).toBe("base");
      expect(result3.viewType).toBe("base");
    });

    it("ignores non-finite coords in aircraft", () => {
      const aircraft = [
        { lat: -33.5, lon: -70.6 },
        { lat: NaN, lon: -70.6 },
        { lat: -33.5, lon: Infinity },
      ];
      const result = computeKioskView(TOBALABA, aircraft, VIEWPORT);
      
      // Should only consider the valid aircraft
      expect(result.viewType).toBe("flights");
    });

    it("ignores non-finite coords in base", () => {
      const base = { lat: NaN, lon: -70.6 };
      const result = computeKioskView(base, [], VIEWPORT);
      
      // Should fall back to fallback
      expect(result.viewType).toBe("fallback");
    });

    it("no base but has flights uses flights bbox", () => {
      const aircraft = [
        { lat: -33.5, lon: -70.6 },
        { lat: -33.4, lon: -70.7 },
      ];
      const result = computeKioskView(null, aircraft, VIEWPORT);
      
      expect(result.viewType).toBe("flights");
      expect(result.centerLat).toBeCloseTo((-33.5 + -33.4) / 2, 5);
      expect(result.centerLon).toBeCloseTo((-70.6 + -70.7) / 2, 5);
    });
  });

  describe("Web-Mercator math", () => {
    it("lonToTileX converts correctly", () => {
      // At zoom 0, the world is 1 tile wide
      expect(lonToTileX(-180, 0)).toBe(0);
      expect(lonToTileX(0, 0)).toBe(0.5);
      expect(lonToTileX(180, 0)).toBe(1);
      
      // At zoom 1, the world is 2 tiles wide
      expect(lonToTileX(-180, 1)).toBe(0);
      expect(lonToTileX(0, 1)).toBe(1);
      expect(lonToTileX(180, 1)).toBe(2);
      
      // Known tile: lon 0, lat 0, z 1 -> x 1, y 1
      expect(lonToTileX(0, 1)).toBe(1);
    });

    it("latToTileY converts correctly", () => {
      // At zoom 0, latitude 0 should be at the middle
      const y0 = latToTileY(0, 0);
      expect(y0).toBeCloseTo(0.5, 5);
      
      // Latitude 85.0511 (approx tanh(1) in mercator) should be near 0
      const yTop = latToTileY(85.0511, 0);
      expect(yTop).toBeCloseTo(0, 5);
      
      // Latitude -85.0511 should be near 1
      const yBottom = latToTileY(-85.0511, 0);
      expect(yBottom).toBeCloseTo(1, 5);
      
      // Known tile: lon 0, lat 0, z 1 -> x 1, y 1
      expect(latToTileY(0, 1)).toBeCloseTo(1, 5);
    });

    it("metersPerPixel computes correctly", () => {
      // At zoom 0, at equator, should be world circumference / 256
      const mpp0 = metersPerPixel(0, 0);
      expect(mpp0).toBeCloseTo(156543.03392, 0);
      
      // At zoom 1, should be half of zoom 0
      const mpp1 = metersPerPixel(0, 1);
      expect(mpp1).toBeCloseTo(mpp0 / 2, 0);
    });
  });

  describe("constants", () => {
    it("DEFAULT_BASE_VIEW_KM is 15", () => {
      expect(DEFAULT_BASE_VIEW_KM).toBe(15);
    });

    it("fallback base is Santiago", () => {
      const fallback = getFallbackBase();
      expect(fallback.lat).toBeCloseTo(-33.4489, 5);
      expect(fallback.lon).toBeCloseTo(-70.6693, 5);
    });

    it("fallback view km is 60", () => {
      expect(getFallbackViewKm()).toBe(60);
    });
  });
});
