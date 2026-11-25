import { describe, expect, it } from "vitest";
import { computeKioskView } from "./kioskMapView";
import {
  computeTileGrid,
  extractPublicAircraft,
  normalizeBaseAerodrome,
  parsePublicMapPayload,
  projectPointToPx,
  renderClubTileMap,
} from "./tileMapRenderer";

const TOBALABA = { icao: "SCTB", lat: -33.456, lon: -70.548 };

describe("tileMapRenderer", () => {
  describe("normalizeBaseAerodrome", () => {
    it("accepts valid ICAO and coordinates", () => {
      expect(normalizeBaseAerodrome({ icao: " SCTB ", lat: -33.456, lon: -70.548 })).toEqual(
        TOBALABA,
      );
    });

    it("allows null ICAO when coordinates are valid", () => {
      expect(normalizeBaseAerodrome({ icao: null, lat: -33.45, lon: -70.55 })).toEqual({
        icao: null,
        lat: -33.45,
        lon: -70.55,
      });
    });

    it("returns null for missing or out-of-range coordinates", () => {
      expect(normalizeBaseAerodrome(null)).toBeNull();
      expect(normalizeBaseAerodrome({ icao: "SCTB", lat: null, lon: -70.5 })).toBeNull();
      expect(normalizeBaseAerodrome({ icao: "SCTB", lat: 95, lon: -70.5 })).toBeNull();
      expect(normalizeBaseAerodrome({ icao: "SCTB", lat: -33.4, lon: -185 })).toBeNull();
      expect(normalizeBaseAerodrome({ icao: "SCTB", lat: Number.NaN, lon: -70.5 })).toBeNull();
    });
  });

  describe("extractPublicAircraft & parsePublicMapPayload", () => {
    it("parses clubName, base, and aircraft filtering malformed entries", () => {
      const parsed = parsePublicMapPayload({
        clubName: "  Club Aéreo de Santiago  ",
        base: { icao: "SCTB", lat: -33.456, lon: -70.548 },
        aircraft: [
          { tail: "CC-ABC", pilotName: "Ana", fix: { lat: -33.5, lon: -70.6 } },
          { tail: "CC-XYZ", fix: { lat: Number.NaN, lon: -70.6 } },
          { tail: "", fix: { lat: -33.5, lon: -70.6 } },
          null,
        ],
      });
      expect(parsed.clubName).toBe("Club Aéreo de Santiago");
      expect(parsed.base).toEqual(TOBALABA);
      expect(parsed.aircraft).toEqual([
        { tail: "CC-ABC", pilotName: "Ana", lat: -33.5, lon: -70.6 },
        { tail: "CC-XYZ", pilotName: undefined, lat: null, lon: null },
      ]);
    });

    it("handles empty or non-object payloads safely", () => {
      expect(parsePublicMapPayload(null)).toEqual({
        clubName: null,
        base: null,
        aircraft: [],
      });
      expect(extractPublicAircraft("bad")).toEqual([]);
    });
  });

  describe("computeTileGrid & projectPointToPx", () => {
    it("generates Esri World_Imagery tiles covering the viewport", () => {
      const viewport = { w: 800, h: 450 };
      const view = computeKioskView(TOBALABA, [], viewport);
      const grid = computeTileGrid(view, viewport);
      expect(grid.zoom).toBeGreaterThanOrEqual(3);
      expect(grid.zoom).toBeLessThanOrEqual(15);
      expect(grid.tiles.length).toBeGreaterThan(0);
      for (const tile of grid.tiles) {
        expect(tile.url).toMatch(
          /^https:\/\/services\.arcgisonline\.com\/ArcGIS\/rest\/services\/World_Imagery\/MapServer\/tile\/\d+\/\d+\/\d+$/,
        );
      }
    });

    it("projects the view center to the exact center of the viewport", () => {
      const viewport = { w: 800, h: 400 };
      const view = computeKioskView(TOBALABA, [], viewport);
      const pt = projectPointToPx(view.centerLat, view.centerLon, view, viewport);
      expect(pt.left).toBeCloseTo(400, 5);
      expect(pt.top).toBeCloseTo(200, 5);
    });
  });

  describe("renderClubTileMap", () => {
    it("renders base view with tiles and base marker when zero flights are active", () => {
      const el = document.createElement("div");
      const view = renderClubTileMap(el, {
        base: TOBALABA,
        aircraft: [],
        viewportPx: { w: 800, h: 450 },
      });
      expect(view.viewType).toBe("base");
      expect(el.getAttribute("data-view")).toBe("base");
      expect(el.querySelectorAll("img[data-tile]").length).toBeGreaterThan(0);
      const baseMarker = el.querySelector('[data-marker="base"]');
      expect(baseMarker?.textContent).toBe("SCTB");
      expect(el.querySelectorAll('[data-marker="aircraft"]').length).toBe(0);
    });

    it("renders fallback view when no base and no flights", () => {
      const el = document.createElement("div");
      const view = renderClubTileMap(el, {
        base: null,
        aircraft: [],
        viewportPx: { w: 800, h: 450 },
      });
      expect(view.viewType).toBe("fallback");
      expect(el.getAttribute("data-view")).toBe("fallback");
      expect(el.querySelectorAll("img[data-tile]").length).toBeGreaterThan(0);
      expect(el.querySelector('[data-marker="base"]')).toBeNull();
    });

    it("renders flights view with both base and aircraft markers", () => {
      const el = document.createElement("div");
      const view = renderClubTileMap(el, {
        base: TOBALABA,
        aircraft: [{ tail: "CC-ABC", pilotName: "Piloto", lat: -33.5, lon: -70.6 }],
        viewportPx: { w: 800, h: 450 },
      });
      expect(view.viewType).toBe("flights");
      expect(el.getAttribute("data-view")).toBe("flights");
      expect(el.querySelector('[data-marker="base"]')?.textContent).toBe("SCTB");
      const acMarker = el.querySelector('[data-marker="aircraft"][data-tail="CC-ABC"]');
      expect(acMarker?.textContent).toBe("CC-ABC");
    });
  });
});
