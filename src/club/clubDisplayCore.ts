import { CHECKIN_STEP_SEC } from "./checkinStep";
import { applyLayoutToEl, type KioskQrLayout } from "./kioskQrLayout";
import { startClubPage } from "./i18nPage";
import {
  computeKioskView,
  latToTileY,
  lonToTileX,
} from "./kioskMapView";
import { t } from "../i18n/index";

export type ClubDisplayOpts = {
  statusEl: HTMLParagraphElement;
  qr: HTMLImageElement;
  qrWrap: HTMLDivElement;
  mapLayer: HTMLElement;
  map: HTMLUListElement;
  viewport: HTMLDivElement;
  activatePanel: HTMLDivElement;
  activateToken: HTMLInputElement;
  activateBtn: HTMLButtonElement;
  /** When set, a local hide/show toggle is shown (not persisted). */
  localToggleBtn?: HTMLButtonElement;
  qrEnabled?: () => boolean;
};

type BaseAerodrome = {
  icao: string | null;
  lat: number | null;
  lon: number | null;
};

type AircraftPosition = {
  tail: string;
  pilotName?: string;
  fix: {
    lat: number;
    lon: number;
    alt?: number;
  };
};

export function mountClubDisplayScreen(opts: ClubDisplayOpts): void {
  const {
    statusEl,
    qr,
    qrWrap,
    mapLayer,
    map,
    viewport,
    activatePanel,
    activateToken,
    activateBtn,
    localToggleBtn,
    qrEnabled,
  } = opts;

  let layout: KioskQrLayout = { x: 0.76, y: 0.76, size: 0.22 };
  let clubQrVisible = true;
  let localQrHidden = false;
  let qrStepTimer: number | undefined;
  let base: BaseAerodrome | null = null;
  let mapContainer: HTMLElement | null = null;

  async function displaySessionActive(): Promise<boolean> {
    const res = await fetch("/api/display/map", { credentials: "same-origin" });
    return res.ok;
  }

  async function activateDisplay(): Promise<boolean> {
    const token = activateToken.value.trim();
    if (!token) return false;
    const res = await fetch("/api/display/activate", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    });
    if (!res.ok) {
      statusEl.textContent = t("kiosk.invalidToken");
      return false;
    }
    activatePanel.hidden = true;
    statusEl.textContent = t("kiosk.activated");
    return true;
  }

  function startQrRefreshLoop(): void {
    window.clearInterval(qrStepTimer);
    qrStepTimer = window.setInterval(() => {
      if (showQr()) refreshQrImage();
    }, CHECKIN_STEP_SEC * 1000);
  }

  function refreshQrImage(): void {
    if (!showQr()) return;
    qr.src = `/api/club/screen/qr.svg?t=${Date.now()}`;
  }

  function showQr(): boolean {
    const enabled = qrEnabled?.() ?? true;
    return enabled && clubQrVisible && !localQrHidden;
  }

  function applyMapLayout(): void {
    mapLayer.classList.toggle("map-expanded", !showQr());
  }

  function paintQr(): void {
    const visible = showQr();
    qrWrap.hidden = !visible;
    applyMapLayout();
    if (visible) {
      applyLayoutToEl(qrWrap, layout, viewport);
    } else {
      qr.removeAttribute("src");
    }
  }

  function syncLocalToggleLabel(): void {
    if (!localToggleBtn) return;
    localToggleBtn.textContent = localQrHidden
      ? t("kiosk.qrShow")
      : t("kiosk.qrHide");
  }

  async function fetchBase(): Promise<BaseAerodrome | null> {
    const res = await fetch("/api/display/base", { credentials: "same-origin" });
    if (!res.ok) return null;
    return (await res.json()) as BaseAerodrome;
  }

  async function refreshMap(): Promise<void> {
    const res = await fetch("/api/display/map", { credentials: "same-origin" });
    if (!res.ok) return;
    const data = (await res.json()) as {
      aircraft: AircraftPosition[];
    };

    // Update the text list (existing behavior)
    map.replaceChildren(
      ...(data.aircraft ?? []).map((row) => {
        const li = document.createElement("li");
        li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
        return li;
      }),
    );

    // Update the tile map
    renderTileMap(data.aircraft);
  }

  function renderTileMap(aircraft: AircraftPosition[]): void {
    if (!mapContainer) {
      // Create the map container if it doesn't exist
      mapContainer = document.createElement("div");
      mapContainer.id = "kiosk-map";
      mapContainer.setAttribute("data-testid", "kiosk-map");
      mapContainer.style.cssText = `
        position: absolute;
        inset: 0;
        overflow: hidden;
      `;
      mapLayer.prepend(mapContainer);

      // Add attribution
      const attribution = document.createElement("div");
      attribution.style.cssText = `
        position: absolute;
        bottom: 0.5rem;
        right: 0.5rem;
        font-size: 0.75rem;
        opacity: 0.7;
        background: rgba(0, 0, 0, 0.5);
        padding: 0.25rem 0.5rem;
        border-radius: 4px;
        pointer-events: none;
      `;
      attribution.textContent = "Esri";
      mapContainer.appendChild(attribution);
    }

    // Clear existing tiles and markers
    const existingTiles = mapContainer.querySelectorAll("img[data-tile]");
    existingTiles.forEach((t) => t.remove());
    const existingMarkers = mapContainer.querySelectorAll("[data-marker]");
    existingMarkers.forEach((m) => m.remove());

    const viewportRect = viewport.getBoundingClientRect();
    const viewportPx = { w: viewportRect.width, h: viewportRect.height };

    // Get aircraft positions (filter out non-finite coords)
    const aircraftPositions = aircraft
      .filter((a) => Number.isFinite(a.fix.lat) && Number.isFinite(a.fix.lon))
      .map((a) => ({ lat: a.fix.lat, lon: a.fix.lon, tail: a.tail }));

    // Compute view
    const baseCoords = base && base.lat !== null && base.lon !== null
      ? { lat: base.lat, lon: base.lon }
      : null;
    
    const view = computeKioskView(baseCoords, aircraftPositions, viewportPx);
    
    // Update data-view attribute
    mapContainer.setAttribute("data-view", view.viewType);

    // Render tiles for the visible area
    renderTiles(view, viewportPx);

    // Render aircraft markers
    for (const ac of aircraftPositions) {
      renderAircraftMarker(ac, view);
    }

    // Render base aerodrome marker if available
    if (base && base.icao && base.lat !== null && base.lon !== null) {
      renderBaseMarker({ lat: base.lat, lon: base.lon, icao: base.icao }, view);
    }
  }

  function renderTiles(view: ReturnType<typeof computeKioskView>, viewportPx: { w: number; h: number }): void {
    const zoom = Math.round(view.zoom);
    const tileSize = 256;
    
    // Remove previous tiles containers
    const existingContainers = mapContainer?.querySelectorAll("[data-tiles]");
    existingContainers?.forEach((c) => c.remove());
    
    // Calculate tile range
    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);
    
    const scale = Math.pow(2, view.zoom - zoom);
    const halfWidthTiles = (viewportPx.w / tileSize) / scale / 2;
    const halfHeightTiles = (viewportPx.h / tileSize) / scale / 2;
    
    const minX = Math.floor(centerX - halfWidthTiles);
    const maxX = Math.ceil(centerX + halfWidthTiles);
    const minY = Math.floor(centerY - halfHeightTiles);
    const maxY = Math.ceil(centerY + halfHeightTiles);
    
    // Create a container for tiles
    const tilesContainer = document.createElement("div");
    tilesContainer.setAttribute("data-tiles", "");
    tilesContainer.style.cssText = `
      position: absolute;
      inset: 0;
      transform-origin: 0 0;
      transform: translate(${viewportPx.w / 2 - scale * centerX * tileSize}px, ${viewportPx.h / 2 - scale * centerY * tileSize}px) scale(${scale});
    `;

    const worldSize = Math.pow(2, zoom);
    
    for (let x = minX; x <= maxX; x++) {
      for (let y = minY; y <= maxY; y++) {
        // Wrap x modulo world size, clamp y to valid range
        const wrappedX = ((x % worldSize) + worldSize) % worldSize;
        const clampedY = Math.max(0, Math.min(y, worldSize - 1));
        
        const tile = document.createElement("img");
        tile.setAttribute("data-tile", "");
        tile.src = `https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/${zoom}/${clampedY}/${wrappedX}`;
        tile.style.cssText = `
          position: absolute;
          left: ${x * tileSize}px;
          top: ${y * tileSize}px;
          width: ${tileSize}px;
          height: ${tileSize}px;
          pointer-events: none;
        `;
        tile.alt = "";
        tilesContainer.appendChild(tile);
      }
    }

    mapContainer?.appendChild(tilesContainer);
  }

  function renderAircraftMarker(ac: { lat: number; lon: number; tail: string }, view: ReturnType<typeof computeKioskView>): void {
    const zoom = view.zoom;
    const tileSize = 256;

    const x = lonToTileX(ac.lon, zoom);
    const y = latToTileY(ac.lat, zoom);

    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);
    
    const marker = document.createElement("div");
    marker.setAttribute("data-marker", "aircraft");
    marker.setAttribute("data-tail", ac.tail);
    marker.style.cssText = `
      position: absolute;
      left: ${(x - centerX) * tileSize + viewport.offsetWidth / 2}px;
      top: ${(y - centerY) * tileSize + viewport.offsetHeight / 2}px;
      transform: translate(-50%, -100%);
      background: #ff5500;
      color: #fff;
      padding: 0.25rem 0.5rem;
      border-radius: 4px;
      font-size: 0.85rem;
      font-weight: 600;
      white-space: nowrap;
      pointer-events: none;
      box-shadow: 0 2px 4px rgba(0,0,0,0.5);
    `;
    marker.textContent = ac.tail;
    mapContainer?.appendChild(marker);
  }

  function renderBaseMarker(base: { lat: number; lon: number; icao: string }, view: ReturnType<typeof computeKioskView>): void {
    const zoom = view.zoom;
    const tileSize = 256;

    const x = lonToTileX(base.lon, zoom);
    const y = latToTileY(base.lat, zoom);

    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);
    
    const marker = document.createElement("div");
    marker.setAttribute("data-marker", "base");
    marker.style.cssText = `
      position: absolute;
      left: ${(x - centerX) * tileSize + viewport.offsetWidth / 2}px;
      top: ${(y - centerY) * tileSize + viewport.offsetHeight / 2}px;
      transform: translate(-50%, -50%);
      width: 24px;
      height: 24px;
      background: #00ff00;
      border: 2px solid #fff;
      border-radius: 50%;
      pointer-events: none;
      box-shadow: 0 2px 4px rgba(0,0,0,0.5);
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 0.65rem;
      font-weight: 600;
      color: #000;
    `;
    marker.textContent = base.icao;
    mapContainer?.appendChild(marker);
  }

  async function fetchLayout(): Promise<void> {
    const res = await fetch("/api/display/kiosk-layout", { credentials: "same-origin" });
    if (!res.ok) return;
    layout = (await res.json()) as KioskQrLayout;
    if (typeof layout.checkinQrVisible === "boolean") {
      clubQrVisible = layout.checkinQrVisible;
    }
    paintQr();
    if (showQr() && !qr.src) refreshQrImage();
  }

  function watchLayoutStream(): void {
    const source = new EventSource("/api/display/kiosk-layout/stream", {
      withCredentials: true,
    });
    source.addEventListener("layout", (ev) => {
      try {
        layout = JSON.parse((ev as MessageEvent).data) as KioskQrLayout;
        if (typeof layout.checkinQrVisible === "boolean") {
          clubQrVisible = layout.checkinQrVisible;
        }
        paintQr();
        if (showQr()) refreshQrImage();
      } catch {
        /* ignore */
      }
    });
    source.onerror = () => {
      source.close();
      window.setTimeout(watchLayoutStream, 3000);
    };
  }

  window.addEventListener("resize", () => {
    paintQr();
    if (mapContainer) {
      void refreshMap();
    }
  });

  activateBtn.addEventListener("click", () => {
    void (async () => {
      if (await activateDisplay()) {
        // Fetch base after activation
        base = await fetchBase();
        refreshQrImage();
        startQrRefreshLoop();
        await fetchLayout();
        await refreshMap();
        watchLayoutStream();
        window.setInterval(() => void fetchLayout(), 8000);
      }
    })();
  });

  localToggleBtn?.addEventListener("click", () => {
    localQrHidden = !localQrHidden;
    syncLocalToggleLabel();
    paintQr();
    if (showQr()) refreshQrImage();
  });

  void (async () => {
    await startClubPage();
    if (!(await displaySessionActive())) {
      activatePanel.hidden = false;
      statusEl.textContent = t("kiosk.pasteToken");
      return;
    }
    activatePanel.hidden = true;
    if (localToggleBtn) {
      localToggleBtn.hidden = false;
      syncLocalToggleLabel();
    }
    
    // Fetch base and layout
    base = await fetchBase();
    await fetchLayout();
    
    statusEl.textContent = t("kiosk.qrRotateHint", { seconds: String(CHECKIN_STEP_SEC) });
    await refreshMap();
    watchLayoutStream();
    window.setInterval(() => void fetchLayout(), 8000);
    startQrRefreshLoop();
    window.setInterval(() => void refreshMap(), 5000);
  })();
}
