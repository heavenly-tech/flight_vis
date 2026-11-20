/**
 * Kiosk map view utilities for the club globe screen.
 * Provides 2D tile map rendering with Web-Mercator (slippy-map) math.
 */

/** Default view distance in kilometers when centered on base. */
export const DEFAULT_BASE_VIEW_KM = 15;

/** Fallback base coordinates (Santiago) when club has no valid base. */
const FALLBACK_BASE = { lat: -33.4489, lon: -70.6693 };

/** Fallback view distance in kilometers (wider view for fallback). */
const FALLBACK_VIEW_KM = 60;

/** Minimum and maximum zoom levels. */
const MIN_ZOOM = 3;
const MAX_ZOOM = 15;

/** Earth's circumference in meters. */
const EARTH_CIRCUMFERENCE = 2 * Math.PI * 6378137;

/**
 * Compute the kiosk map view based on base and aircraft positions.
 * Returns center coordinates and zoom level.
 */
export function computeKioskView(
  base: { lat: number; lon: number } | null,
  aircraft: Array<{ lat: number; lon: number }>,
  viewportPx: { w: number; h: number },
): { centerLat: number; centerLon: number; zoom: number; viewType: "base" | "flights" | "fallback" } {
  // Filter out non-finite coordinates
  const validAircraft = aircraft.filter(
    (a) => Number.isFinite(a.lat) && Number.isFinite(a.lon),
  );

  // If we have a valid base
  if (base && Number.isFinite(base.lat) && Number.isFinite(base.lon)) {
    // No aircraft: center on base
    if (validAircraft.length === 0) {
      return {
        centerLat: base.lat,
        centerLon: base.lon,
        zoom: computeZoomForDistance(DEFAULT_BASE_VIEW_KM, viewportPx.w),
        viewType: "base",
      };
    }

    // With aircraft: fit bounding box including base
    const allPoints = [base, ...validAircraft];
    const bbox = computeBoundingBox(allPoints);
    const { centerLat, centerLon, spanMeters } = bbox;
    const zoom = computeZoomForDistance(spanMeters / 1000, viewportPx.w);
    
    return {
      centerLat,
      centerLon,
      zoom: Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom)),
      viewType: "flights",
    };
  }

  // No valid base
  if (validAircraft.length === 0) {
    // No base, no aircraft: use fallback
    return {
      centerLat: FALLBACK_BASE.lat,
      centerLon: FALLBACK_BASE.lon,
      zoom: computeZoomForDistance(FALLBACK_VIEW_KM, viewportPx.w),
      viewType: "fallback",
    };
  }

  // No base but has aircraft: fit aircraft bounding box
  const bbox = computeBoundingBox(validAircraft);
  const { centerLat, centerLon, spanMeters } = bbox;
  const zoom = computeZoomForDistance(spanMeters / 1000, viewportPx.w);
  
  return {
    centerLat,
    centerLon,
    zoom: Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom)),
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

  // Compute span in meters using haversine approximation
  // For small distances, we can approximate using lat/lon degrees
  const latSpanDeg = maxLat - minLat;
  const lonSpanDeg = maxLon - minLon;
  
  // Approximate meters per degree at center latitude
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

  // At zoom level z, the world is 256 * 2^z pixels wide
  // The distance shown is (world_circumference / (256 * 2^z)) * viewportWidthPx
  // Solving for z: z = log2(world_circumference / (distance * 256 / viewportWidthPx))
  
  const worldCircumferenceKm = EARTH_CIRCUMFERENCE / 1000;
  const metersPerPixel = (distanceKm * 1000) / viewportWidthPx;
  const zoom = Math.log2(worldCircumferenceKm * 1000 / (metersPerPixel * 256));
  
  return zoom;
}

/**
 * Convert longitude to tile X coordinate at a given zoom level.
 */
export function lonToTileX(lon: number, zoom: number): number {
  return ((lon + 180) / 360) * Math.pow(2, zoom);
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
 * Compute meters per pixel at a given latitude and zoom level.
 */
export function metersPerPixel(lat: number, zoom: number): number {
  const latRad = (lat * Math.PI) / 180;
  const metersPerPixelEquator = EARTH_CIRCUMFERENCE / (256 * Math.pow(2, zoom));
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
