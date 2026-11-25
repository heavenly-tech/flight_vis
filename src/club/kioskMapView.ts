/**
 * Kiosk map view utilities for club live screens.
 * Provides 2D tile map rendering with Web-Mercator (slippy-map) math.
 */

/** Default view distance in kilometers when centered on base. */
export const DEFAULT_BASE_VIEW_KM = 15;

/** Default view distance in kilometers when focused on a single aircraft. */
export const DEFAULT_FOCUS_VIEW_KM = 5;

/** Fallback base coordinates (Santiago) when club has no valid base. */
const FALLBACK_BASE = { lat: -33.4489, lon: -70.6693 };

/** Fallback view distance in kilometers (wider view for fallback). */
const FALLBACK_VIEW_KM = 60;

/** Minimum and maximum zoom levels. */
const MIN_ZOOM = 3;
const MAX_ZOOM = 15;

/** Earth's circumference in meters. */
const EARTH_CIRCUMFERENCE = 2 * Math.PI * 6378137;

const TILE_SIZE = 256;

export type KioskViewPadding = {
  top?: number;
  right?: number;
  bottom?: number;
  left?: number;
};

export type KioskViewOptions = {
  /** When set with finite coordinates, center on this target at closer zoom (~5 km span). */
  focus?: { lat: number; lon: number } | null;
  /** Override span in km for focus mode (defaults to DEFAULT_FOCUS_VIEW_KM = 5). */
  focusViewKm?: number;
  /** Safe inset padding in pixels (e.g. to keep markers clear of top header/controls). */
  padding?: KioskViewPadding;
};

export type KioskViewResult = {
  centerLat: number;
  centerLon: number;
  zoom: number;
  viewType: "base" | "flights" | "fallback" | "focus";
};

type NormalizedPadding = {
  top: number;
  right: number;
  bottom: number;
  left: number;
  safeW: number;
  safeH: number;
};

function clampZoom(zoom: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom));
}

function normalizePadding(
  padding: KioskViewPadding | undefined,
  viewportPx: { w: number; h: number },
): NormalizedPadding {
  const w = Math.max(1, Number.isFinite(viewportPx.w) ? viewportPx.w : 1);
  const h = Math.max(1, Number.isFinite(viewportPx.h) ? viewportPx.h : 1);
  let top = Math.max(0, Number.isFinite(padding?.top) ? (padding?.top ?? 0) : 0);
  let bottom = Math.max(0, Number.isFinite(padding?.bottom) ? (padding?.bottom ?? 0) : 0);
  let left = Math.max(0, Number.isFinite(padding?.left) ? (padding?.left ?? 0) : 0);
  let right = Math.max(0, Number.isFinite(padding?.right) ? (padding?.right ?? 0) : 0);

  if (left + right >= w * 0.85) {
    const scale = (w * 0.85) / (left + right);
    left *= scale;
    right *= scale;
  }
  if (top + bottom >= h * 0.85) {
    const scale = (h * 0.85) / (top + bottom);
    top *= scale;
    bottom *= scale;
  }

  return {
    top,
    right,
    bottom,
    left,
    safeW: Math.max(1, w - left - right),
    safeH: Math.max(1, h - top - bottom),
  };
}

function offsetCenterForPadding(
  targetLat: number,
  targetLon: number,
  zoom: number,
  pad: NormalizedPadding,
): { centerLat: number; centerLon: number } {
  if (pad.top === pad.bottom && pad.left === pad.right) {
    return { centerLat: targetLat, centerLon: targetLon };
  }
  const worldTiles = Math.pow(2, zoom);
  const x = lonToTileX(targetLon, zoom) - (pad.left - pad.right) / (2 * TILE_SIZE);
  const y = latToTileY(targetLat, zoom) - (pad.top - pad.bottom) / (2 * TILE_SIZE);
  const minY = 1e-6 * worldTiles;
  const maxY = (1 - 1e-6) * worldTiles;
  const clampedY = Math.max(minY, Math.min(maxY, y));
  return {
    centerLat: tileYToLat(clampedY, zoom),
    centerLon: tileXToLon(x, zoom),
  };
}

function computeFitZoom(
  points: Array<{ lat: number; lon: number }>,
  targetLat: number,
  targetLon: number,
  safeW: number,
  safeH: number,
): number {
  const targetX0 = lonToTileX(targetLon, 0);
  const targetY0 = latToTileY(targetLat, 0);
  let maxDx0 = 0;
  let maxDy0 = 0;
  for (const p of points) {
    maxDx0 = Math.max(maxDx0, Math.abs(lonToTileX(p.lon, 0) - targetX0));
    maxDy0 = Math.max(maxDy0, Math.abs(latToTileY(p.lat, 0) - targetY0));
  }
  const spanX0 = maxDx0 * 2;
  const spanY0 = maxDy0 * 2;
  const zoomX = spanX0 > 0 && safeW > 0 ? Math.log2(safeW / (spanX0 * TILE_SIZE)) : MAX_ZOOM;
  const zoomY = spanY0 > 0 && safeH > 0 ? Math.log2(safeH / (spanY0 * TILE_SIZE)) : MAX_ZOOM;
  return clampZoom(Math.min(zoomX, zoomY));
}

/**
 * Compute the kiosk map view based on base, aircraft positions, and optional focus/padding.
 * Returns center coordinates and zoom level.
 */
export function computeKioskView(
  base: { lat: number; lon: number } | null,
  aircraft: Array<{ lat: number; lon: number }>,
  viewportPx: { w: number; h: number },
  options?: KioskViewOptions,
): KioskViewResult {
  const pad = normalizePadding(options?.padding, viewportPx);

  // Explicit focus target takes precedence (e.g. user tapped a tail)
  const focus = options?.focus;
  if (focus && Number.isFinite(focus.lat) && Number.isFinite(focus.lon)) {
    const focusKm =
      options?.focusViewKm != null &&
      Number.isFinite(options.focusViewKm) &&
      options.focusViewKm > 0
        ? options.focusViewKm
        : DEFAULT_FOCUS_VIEW_KM;
    const zoom = clampZoom(computeZoomForDistance(focusKm, pad.safeW));
    const { centerLat, centerLon } = offsetCenterForPadding(focus.lat, focus.lon, zoom, pad);
    return {
      centerLat,
      centerLon,
      zoom,
      viewType: "focus",
    };
  }

  // Filter out non-finite coordinates
  const validAircraft = aircraft.filter(
    (a) => Number.isFinite(a.lat) && Number.isFinite(a.lon),
  );

  // If we have a valid base
  if (base && Number.isFinite(base.lat) && Number.isFinite(base.lon)) {
    // No aircraft: center on base
    if (validAircraft.length === 0) {
      const zoom = clampZoom(computeZoomForDistance(DEFAULT_BASE_VIEW_KM, pad.safeW));
      const { centerLat, centerLon } = offsetCenterForPadding(base.lat, base.lon, zoom, pad);
      return {
        centerLat,
        centerLon,
        zoom,
        viewType: "base",
      };
    }

    // With aircraft: fit bounding box including base
    const allPoints = [base, ...validAircraft];
    const bbox = computeBoundingBox(allPoints);
    const zoom = computeFitZoom(allPoints, bbox.centerLat, bbox.centerLon, pad.safeW, pad.safeH);
    const { centerLat, centerLon } = offsetCenterForPadding(
      bbox.centerLat,
      bbox.centerLon,
      zoom,
      pad,
    );

    return {
      centerLat,
      centerLon,
      zoom,
      viewType: "flights",
    };
  }

  // No valid base
  if (validAircraft.length === 0) {
    // No base, no aircraft: use fallback
    const zoom = clampZoom(computeZoomForDistance(FALLBACK_VIEW_KM, pad.safeW));
    const { centerLat, centerLon } = offsetCenterForPadding(
      FALLBACK_BASE.lat,
      FALLBACK_BASE.lon,
      zoom,
      pad,
    );
    return {
      centerLat,
      centerLon,
      zoom,
      viewType: "fallback",
    };
  }

  // No base but has aircraft: fit aircraft bounding box
  const bbox = computeBoundingBox(validAircraft);
  const zoom = computeFitZoom(validAircraft, bbox.centerLat, bbox.centerLon, pad.safeW, pad.safeH);
  const { centerLat, centerLon } = offsetCenterForPadding(
    bbox.centerLat,
    bbox.centerLon,
    zoom,
    pad,
  );

  return {
    centerLat,
    centerLon,
    zoom,
    viewType: "flights",
  };
}

/**
 * Compute bounding box from a list of points.
 * Returns center coordinates and span in meters.
 */
function computeBoundingBox(
  points: Array<{ lat: number; lon: number }>,
): { centerLat: number; centerLon: number; spanMeters: number } {
  if (points.length === 0) {
    return { centerLat: 0, centerLon: 0, spanMeters: 0 };
  }

  let minLat = Infinity;
  let maxLat = -Infinity;
  let minLon = Infinity;
  let maxLon = -Infinity;

  for (const p of points) {
    minLat = Math.min(minLat, p.lat);
    maxLat = Math.max(maxLat, p.lat);
    minLon = Math.min(minLon, p.lon);
    maxLon = Math.max(maxLon, p.lon);
  }

  const centerLat = (minLat + maxLat) / 2;
  const centerLon = (minLon + maxLon) / 2;

  const latSpanDeg = maxLat - minLat;
  const lonSpanDeg = maxLon - minLon;

  const metersPerLatDeg = EARTH_CIRCUMFERENCE / 360;
  const metersPerLonDeg = metersPerLatDeg * Math.cos((centerLat * Math.PI) / 180);

  const spanMeters = Math.max(
    latSpanDeg * metersPerLatDeg,
    lonSpanDeg * metersPerLonDeg,
  );

  return { centerLat, centerLon, spanMeters };
}

/**
 * Compute zoom level for a given distance (km) and viewport width (px).
 * Uses Web-Mercator tile math.
 */
function computeZoomForDistance(distanceKm: number, viewportWidthPx: number): number {
  if (distanceKm <= 0 || viewportWidthPx <= 0) {
    return MAX_ZOOM;
  }

  const worldCircumferenceKm = EARTH_CIRCUMFERENCE / 1000;
  const metersPerPixel = (distanceKm * 1000) / viewportWidthPx;
  const zoom = Math.log2((worldCircumferenceKm * 1000) / (metersPerPixel * TILE_SIZE));

  return zoom;
}

/**
 * Convert longitude to tile X coordinate at a given zoom level.
 */
export function lonToTileX(lon: number, zoom: number): number {
  return ((lon + 180) / 360) * Math.pow(2, zoom);
}

/**
 * Convert tile X coordinate back to longitude at a given zoom level.
 */
export function tileXToLon(tileX: number, zoom: number): number {
  return (tileX / Math.pow(2, zoom)) * 360 - 180;
}

/**
 * Convert latitude to tile Y coordinate at a given zoom level.
 */
export function latToTileY(lat: number, zoom: number): number {
  const latRad = (lat * Math.PI) / 180;
  const mercator = Math.log(Math.tan(latRad) + 1 / Math.cos(latRad)) / Math.PI;
  return ((1 - mercator) / 2) * Math.pow(2, zoom);
}

/**
 * Convert tile Y coordinate back to latitude at a given zoom level.
 */
export function tileYToLat(tileY: number, zoom: number): number {
  const n = Math.PI * (1 - (2 * tileY) / Math.pow(2, zoom));
  return (Math.atan(Math.sinh(n)) * 180) / Math.PI;
}

/**
 * Compute meters per pixel at a given latitude and zoom level.
 */
export function metersPerPixel(lat: number, zoom: number): number {
  const latRad = (lat * Math.PI) / 180;
  const metersPerPixelEquator = EARTH_CIRCUMFERENCE / (TILE_SIZE * Math.pow(2, zoom));
  return metersPerPixelEquator / Math.cos(latRad);
}

/**
 * Get the fallback base coordinates.
 */
export function getFallbackBase(): { lat: number; lon: number } {
  return { ...FALLBACK_BASE };
}

/**
 * Get the fallback view distance in km.
 */
export function getFallbackViewKm(): number {
  return FALLBACK_VIEW_KM;
}
