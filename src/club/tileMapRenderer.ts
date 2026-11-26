/**
 * Shared 2D Web-Mercator tile map renderer reusing kioskMapView math.
 * Used by public-map.html so it matches the kiosk map behavior (base aerodrome,
 * ~15 km view, fallback when no base, fit to active flights + base).
 */
import { computeKioskView, latToTileY, lonToTileX } from "./kioskMapView";
import {
  DEFAULT_TRAIL_MINUTES,
  DEFAULT_TRAIL_RGB,
  NON_CLUB_TRAIL_RGB,
  isValidTrailMinutes,
  matchTrailForAircraft,
  parseTrailMinutes,
  prepareTrailPoints,
  renderTrailsOverlay,
  type FlightTrail,
  type RenderTrailItem,
  type TrailMinutes,
} from "./liveTrail";
import { t } from "../i18n/index";

const TILE_SIZE = 256;
const MIN_TILE_ZOOM = 3;
const MAX_TILE_ZOOM = 15;

export type RawBaseAerodrome = {
  icao?: string | null;
  lat?: number | null;
  lon?: number | null;
};

export type NormalizedBase = {
  icao: string | null;
  lat: number;
  lon: number;
};

export type RawPublicAircraft = {
  tail?: unknown;
  flightId?: unknown;
  pilotName?: unknown;
  trailMinutes?: unknown;
  nonClub?: unknown;
  fix?: {
    t?: unknown;
    lat?: unknown;
    lon?: unknown;
    alt?: unknown;
  } | null;
};

export type PublicMapAircraft = {
  tail: string;
  flightId?: string;
  pilotName?: string;
  lat: number | null;
  lon: number | null;
  fixT?: number;
  trailMinutes?: TrailMinutes;
  nonClub?: boolean;
};

export type PublicMapPayload = {
  clubName: string | null;
  base: NormalizedBase | null;
  aircraft: PublicMapAircraft[];
};

export type TileSpec = {
  x: number;
  y: number;
  wrappedX: number;
  clampedY: number;
  url: string;
};

export type TileGrid = {
  zoom: number;
  scale: number;
  centerX: number;
  centerY: number;
  translateX: number;
  translateY: number;
  tiles: TileSpec[];
};

export function normalizeBaseAerodrome(raw: RawBaseAerodrome | null | undefined): NormalizedBase | null {
  if (!raw || typeof raw !== "object") return null;
  const lat = raw.lat;
  const lon = raw.lon;
  if (
    typeof lat !== "number" ||
    typeof lon !== "number" ||
    !Number.isFinite(lat) ||
    !Number.isFinite(lon) ||
    lat < -90 ||
    lat > 90 ||
    lon < -180 ||
    lon > 180
  ) {
    return null;
  }
  const icao = typeof raw.icao === "string" && raw.icao.trim().length > 0 ? raw.icao.trim() : null;
  return { icao, lat, lon };
}

export function extractPublicAircraft(rows: unknown): PublicMapAircraft[] {
  if (!Array.isArray(rows)) return [];
  const out: PublicMapAircraft[] = [];
  for (const item of rows as RawPublicAircraft[]) {
    if (!item || typeof item !== "object" || typeof item.tail !== "string" || !item.tail.trim()) {
      continue;
    }
    const pilotName =
      typeof item.pilotName === "string" && item.pilotName.trim().length > 0
        ? item.pilotName.trim()
        : undefined;
    const rawLat = item.fix?.lat;
    const rawLon = item.fix?.lon;
    const hasCoords =
      typeof rawLat === "number" &&
      typeof rawLon === "number" &&
      Number.isFinite(rawLat) &&
      Number.isFinite(rawLon) &&
      rawLat >= -90 &&
      rawLat <= 90 &&
      rawLon >= -180 &&
      rawLon <= 180;
    const entry: PublicMapAircraft = {
      tail: item.tail.trim(),
      pilotName,
      lat: hasCoords ? rawLat : null,
      lon: hasCoords ? rawLon : null,
    };
    if (typeof item.flightId === "string" && item.flightId.trim().length > 0) {
      entry.flightId = item.flightId.trim();
    }
    if (typeof item.fix?.t === "number" && Number.isFinite(item.fix.t)) {
      entry.fixT = item.fix.t;
    }
    if (isValidTrailMinutes(item.trailMinutes)) {
      entry.trailMinutes = item.trailMinutes;
    }
    if (item.nonClub === true) {
      entry.nonClub = true;
    }
    out.push(entry);
  }
  return out;
}

export function parsePublicMapPayload(raw: unknown): PublicMapPayload {
  if (!raw || typeof raw !== "object") {
    return { clubName: null, base: null, aircraft: [] };
  }
  const rec = raw as {
    clubName?: unknown;
    base?: RawBaseAerodrome | null;
    aircraft?: unknown;
  };
  const clubName =
    typeof rec.clubName === "string" && rec.clubName.trim().length > 0
      ? rec.clubName.trim()
      : null;
  const base = normalizeBaseAerodrome(rec.base);
  const aircraft = extractPublicAircraft(rec.aircraft);
  return { clubName, base, aircraft };
}

export function computeTileGrid(
  view: ReturnType<typeof computeKioskView>,
  viewportPx: { w: number; h: number },
): TileGrid {
  const clampedZoom = Math.max(MIN_TILE_ZOOM, Math.min(MAX_TILE_ZOOM, view.zoom));
  const zoom = Math.max(MIN_TILE_ZOOM, Math.min(MAX_TILE_ZOOM, Math.round(clampedZoom)));
  const centerX = lonToTileX(view.centerLon, zoom);
  const centerY = latToTileY(view.centerLat, zoom);
  const scale = Math.pow(2, clampedZoom - zoom);
  const halfWidthTiles = viewportPx.w / TILE_SIZE / scale / 2;
  const halfHeightTiles = viewportPx.h / TILE_SIZE / scale / 2;

  const minX = Math.floor(centerX - halfWidthTiles);
  const maxX = Math.ceil(centerX + halfWidthTiles);
  const minY = Math.floor(centerY - halfHeightTiles);
  const maxY = Math.ceil(centerY + halfHeightTiles);
  const worldSize = Math.pow(2, zoom);

  const tiles: TileSpec[] = [];
  for (let x = minX; x <= maxX; x++) {
    for (let y = minY; y <= maxY; y++) {
      const wrappedX = ((x % worldSize) + worldSize) % worldSize;
      const clampedY = Math.max(0, Math.min(y, worldSize - 1));
      tiles.push({
        x,
        y,
        wrappedX,
        clampedY,
        url: `https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/${zoom}/${clampedY}/${wrappedX}`,
      });
    }
  }

  return {
    zoom,
    scale,
    centerX,
    centerY,
    translateX: viewportPx.w / 2 - scale * centerX * TILE_SIZE,
    translateY: viewportPx.h / 2 - scale * centerY * TILE_SIZE,
    tiles,
  };
}

export function projectPointToPx(
  lat: number,
  lon: number,
  view: ReturnType<typeof computeKioskView>,
  viewportPx: { w: number; h: number },
): { left: number; top: number } {
  const clampedZoom = Math.max(MIN_TILE_ZOOM, Math.min(MAX_TILE_ZOOM, view.zoom));
  const x = lonToTileX(lon, clampedZoom);
  const y = latToTileY(lat, clampedZoom);
  const centerX = lonToTileX(view.centerLon, clampedZoom);
  const centerY = latToTileY(view.centerLat, clampedZoom);
  return {
    left: (x - centerX) * TILE_SIZE + viewportPx.w / 2,
    top: (y - centerY) * TILE_SIZE + viewportPx.h / 2,
  };
}

export function renderClubTileMap(
  container: HTMLElement,
  opts: {
    base: NormalizedBase | null;
    aircraft: PublicMapAircraft[];
    trails?: readonly FlightTrail[];
    trailLineWidthPx?: number;
    viewportPx?: { w: number; h: number };
  },
): ReturnType<typeof computeKioskView> {
  const rect = container.getBoundingClientRect();
  const viewportPx = opts.viewportPx ?? {
    w: rect.width > 0 ? rect.width : container.clientWidth || 800,
    h: rect.height > 0 ? rect.height : container.clientHeight || 450,
  };

  const validAircraft = opts.aircraft.filter(
    (a): a is PublicMapAircraft & { lat: number; lon: number } =>
      a.lat !== null && a.lon !== null && Number.isFinite(a.lat) && Number.isFinite(a.lon),
  );
  const baseCoords = opts.base ? { lat: opts.base.lat, lon: opts.base.lon } : null;
  const view = computeKioskView(
    baseCoords,
    validAircraft,
    viewportPx,
    validAircraft.length > 0
      ? { padding: { top: 44, right: 56, bottom: 24, left: 56 } }
      : undefined,
  );

  container.setAttribute("data-view", view.viewType);

  container
    .querySelectorAll("[data-tiles], [data-marker], [data-trails-canvas], [data-trails-layer]")
    .forEach((el) => el.remove());

  const grid = computeTileGrid(view, viewportPx);
  const tilesContainer = document.createElement("div");
  tilesContainer.setAttribute("data-tiles", "");
  tilesContainer.className = "tile-map-layer";
  tilesContainer.style.transform = `translate(${grid.translateX}px, ${grid.translateY}px) scale(${grid.scale})`;

  for (const spec of grid.tiles) {
    const img = document.createElement("img");
    img.setAttribute("data-tile", "");
    img.className = "tile-map-img";
    img.src = spec.url;
    img.alt = "";
    img.style.left = `${spec.x * TILE_SIZE}px`;
    img.style.top = `${spec.y * TILE_SIZE}px`;
    img.style.width = `${TILE_SIZE}px`;
    img.style.height = `${TILE_SIZE}px`;
    tilesContainer.appendChild(img);
  }
  container.prepend(tilesContainer);

  if (opts.trails && opts.trails.length > 0 && validAircraft.length > 0) {
    const trailItems: RenderTrailItem[] = [];
    for (const ac of validAircraft) {
      const matched = matchTrailForAircraft(opts.trails, ac);
      if (!matched) continue;
      const effectiveMinutes = parseTrailMinutes(
        ac.trailMinutes ?? matched.trailMinutes,
        DEFAULT_TRAIL_MINUTES,
      );
      const pts = prepareTrailPoints(matched.points, effectiveMinutes, {
        t: ac.fixT,
        lat: ac.lat,
        lon: ac.lon,
      });
      if (pts.length >= 2) {
        const isNonClub = ac.nonClub === true || matched.nonClub === true;
        trailItems.push({
          flightId: matched.flightId || ac.flightId || "",
          tail: ac.tail,
          trailMinutes: effectiveMinutes,
          nonClub: isNonClub || undefined,
          points: pts,
          rgb: isNonClub ? NON_CLUB_TRAIL_RGB : DEFAULT_TRAIL_RGB,
        });
      }
    }
    renderTrailsOverlay(container, {
      viewportPx,
      lineWidthPx: opts.trailLineWidthPx ?? 3,
      items: trailItems,
      project: (lat, lon) => projectPointToPx(lat, lon, view, viewportPx),
    });
  }

  // Render non-club markers first so own-club aircraft stay visually on top.
  const orderedMarkers = [
    ...validAircraft.filter((ac) => ac.nonClub === true),
    ...validAircraft.filter((ac) => ac.nonClub !== true),
  ];

  for (const ac of orderedMarkers) {
    const pos = projectPointToPx(ac.lat, ac.lon, view, viewportPx);
    const marker = document.createElement("div");
    marker.setAttribute("data-marker", "aircraft");
    marker.setAttribute("data-tail", ac.tail);
    marker.style.left = `${pos.left}px`;
    marker.style.top = `${pos.top}px`;
    if (ac.nonClub === true) {
      const otherClubLabel = t("common.otherClub");
      marker.className = "tile-map-aircraft is-non-club";
      marker.setAttribute("data-non-club", "true");
      const tailSpan = document.createElement("span");
      tailSpan.className = "marker-tail";
      tailSpan.textContent = ac.tail;
      const badge = document.createElement("span");
      badge.className = "non-club-tag";
      badge.setAttribute("data-non-club-label", "true");
      badge.textContent = otherClubLabel;
      marker.append(tailSpan, badge);
      marker.title = ac.pilotName
        ? `${ac.tail} · ${otherClubLabel} · ${ac.pilotName}`
        : `${ac.tail} · ${otherClubLabel}`;
    } else {
      marker.className = "tile-map-aircraft";
      marker.textContent = ac.tail;
      if (ac.pilotName) {
        marker.title = `${ac.tail} · ${ac.pilotName}`;
      }
    }
    container.appendChild(marker);
  }

  if (opts.base) {
    const pos = projectPointToPx(opts.base.lat, opts.base.lon, view, viewportPx);
    const marker = document.createElement("div");
    marker.setAttribute("data-marker", "base");
    marker.className = "tile-map-base";
    marker.style.left = `${pos.left}px`;
    marker.style.top = `${pos.top}px`;
    marker.textContent = opts.base.icao ?? "";
    if (opts.base.icao) {
      marker.setAttribute("data-icao", opts.base.icao);
    }
    container.appendChild(marker);
  }

  if (!container.querySelector("[data-attribution]")) {
    const attr = document.createElement("div");
    attr.setAttribute("data-attribution", "");
    attr.className = "tile-map-attribution";
    attr.textContent = "Esri";
    container.appendChild(attr);
  }

  return view;
}
