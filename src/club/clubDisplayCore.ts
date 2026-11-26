import { CHECKIN_STEP_SEC } from "./checkinStep";
import { applyLayoutToEl, type KioskQrLayout } from "./kioskQrLayout";
import { startClubPage } from "./i18nPage";
import {
  computeKioskView,
  latToTileY,
  lonToTileX,
  type KioskViewPadding,
} from "./kioskMapView";
import { subscribeKioskSignals } from "./kioskSync";
import { clubStaffLoginRedirectUrl } from "./safeNext";
import { t } from "../i18n/index";

export type ClubDisplayOpts = {
  statusEl: HTMLParagraphElement;
  qr: HTMLImageElement;
  qrWrap: HTMLDivElement;
  mapLayer: HTMLElement;
  map: HTMLUListElement;
  viewport: HTMLDivElement;
  /** When set, a local hide/show toggle is shown (not persisted). */
  localToggleBtn?: HTMLButtonElement;
  clubNameEl?: HTMLElement;
  viewAllBtn?: HTMLButtonElement;
  emptyEl?: HTMLElement;
  focusDetailEl?: HTMLElement;
  qrEnabled?: () => boolean;
  /** Override login redirect navigation (for unit tests). */
  onRedirectToLogin?: (url: string) => void;
};

type BaseAerodrome = {
  icao: string | null;
  lat: number | null;
  lon: number | null;
  clubName?: string | null;
};

type AircraftPosition = {
  tail: string;
  pilotName?: string | null;
  fix?: {
    lat: number;
    lon: number;
    alt?: number | null;
    speed?: number | null;
    course?: number | null;
    t?: number;
  } | null;
};

export function mountClubDisplayScreen(opts: ClubDisplayOpts): void {
  const {
    statusEl,
    qr,
    qrWrap,
    mapLayer,
    map,
    viewport,
    localToggleBtn,
    qrEnabled,
  } = opts;
  const onRedirectToLogin =
    opts.onRedirectToLogin ?? ((url: string) => window.location.replace(url));
  const kioskLoginUrl = clubStaffLoginRedirectUrl("/club-screen.html");

  const clubNameEl = resolveClubNameEl(opts.clubNameEl);
  const viewAllBtn = resolveViewAllBtn(opts.viewAllBtn);
  const emptyEl = resolveEmptyEl(opts.emptyEl);
  const focusDetailEl = resolveFocusDetailEl(opts.focusDetailEl);

  let layout: KioskQrLayout = { x: 0.76, y: 0.76, size: 0.22 };
  let clubQrVisible = true;
  let localQrHidden = false;
  let qrStepTimer: number | undefined;
  let layoutPollTimer: number | undefined;
  let mapPollTimer: number | undefined;
  let sseReconnectTimer: number | undefined;
  let refetchDebounceTimer: number | undefined;
  let layoutSource: EventSource | null = null;
  let unsubscribeSignals: (() => void) | null = null;
  let activeLoopStarted = false;
  let sessionEnded = false;
  let base: BaseAerodrome | null = null;
  let mapContainer: HTMLElement | null = null;
  let lastAircraft: AircraftPosition[] = [];
  let focusedTail: string | null = null;

  function redirectToLoginOnce(): void {
    if (sessionEnded) return;
    sessionEnded = true;
    window.clearInterval(qrStepTimer);
    window.clearInterval(layoutPollTimer);
    window.clearInterval(mapPollTimer);
    window.clearTimeout(sseReconnectTimer);
    window.clearTimeout(refetchDebounceTimer);
    if (layoutSource) {
      layoutSource.close();
      layoutSource = null;
    }
    if (unsubscribeSignals) {
      unsubscribeSignals();
      unsubscribeSignals = null;
    }
    onRedirectToLogin(kioskLoginUrl);
  }

  function isAuthFailure(status: number): boolean {
    return status === 401 || status === 403;
  }

  function resolveClubNameEl(provided?: HTMLElement): HTMLElement | null {
    if (provided) return provided;
    const existing = viewport.querySelector<HTMLElement>("#club-name");
    if (existing) return existing;
    const headerMain = viewport.querySelector<HTMLElement>(".kiosk-header-main");
    if (!headerMain) return null;
    const el = document.createElement("p");
    el.id = "club-name";
    el.className = "kiosk-club-name";
    el.hidden = true;
    headerMain.prepend(el);
    return el;
  }

  function resolveViewAllBtn(provided?: HTMLButtonElement): HTMLButtonElement | null {
    if (provided) return provided;
    const existing = viewport.querySelector<HTMLButtonElement>("#btn-view-all");
    if (existing) return existing;
    const toolbar = viewport.querySelector<HTMLElement>("#toolbar-local");
    if (!toolbar) return null;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.id = "btn-view-all";
    btn.className = "btn-secondary";
    btn.hidden = true;
    toolbar.prepend(btn);
    return btn;
  }

  function resolveEmptyEl(provided?: HTMLElement): HTMLElement {
    if (provided) return provided;
    const existing = viewport.querySelector<HTMLElement>("#map-empty");
    if (existing) return existing;
    const el = document.createElement("p");
    el.id = "map-empty";
    el.className = "kiosk-empty";
    el.setAttribute("role", "status");
    el.setAttribute("aria-live", "polite");
    el.hidden = true;
    map.parentElement?.insertBefore(el, map);
    return el;
  }

  function resolveFocusDetailEl(provided?: HTMLElement): HTMLElement {
    if (provided) return provided;
    const existing = viewport.querySelector<HTMLElement>("#focus-detail");
    if (existing) return existing;
    const el = document.createElement("div");
    el.id = "focus-detail";
    el.className = "kiosk-focus-detail";
    el.setAttribute("role", "region");
    el.setAttribute("aria-live", "polite");
    el.hidden = true;
    map.insertAdjacentElement("afterend", el);
    return el;
  }

  function syncClubName(): void {
    if (!clubNameEl) return;
    const name = typeof base?.clubName === "string" ? base.clubName.trim() : "";
    if (name) {
      clubNameEl.textContent = name;
      clubNameEl.hidden = false;
    } else {
      clubNameEl.textContent = "";
      clubNameEl.hidden = true;
    }
  }

  function toggleFocusTail(tail: string): void {
    focusedTail = focusedTail === tail ? null : tail;
    renderCurrentState();
  }

  function clearFocus(): void {
    if (focusedTail === null) return;
    focusedTail = null;
    renderCurrentState();
  }

  function startQrRefreshLoop(): void {
    window.clearInterval(qrStepTimer);
    qrStepTimer = window.setInterval(() => {
      if (!sessionEnded && showQr()) refreshQrImage();
    }, CHECKIN_STEP_SEC * 1000);
  }

  function refreshQrImage(): void {
    if (sessionEnded || !showQr()) return;
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

  function syncViewAllButton(): void {
    if (!viewAllBtn) return;
    viewAllBtn.textContent = t("kiosk.viewAll");
    viewAllBtn.hidden = focusedTail === null;
  }

  async function refreshBase(): Promise<boolean> {
    if (sessionEnded) return false;
    const res = await fetch("/api/display/base", { credentials: "same-origin" });
    if (isAuthFailure(res.status)) {
      redirectToLoginOnce();
      return false;
    }
    if (!res.ok) return false;
    base = (await res.json()) as BaseAerodrome;
    syncClubName();
    return true;
  }

  function renderAircraftList(aircraft: AircraftPosition[]): void {
    const activeChipTail =
      document.activeElement instanceof HTMLButtonElement &&
      map.contains(document.activeElement)
        ? document.activeElement.dataset.tail ?? null
        : null;

    if (aircraft.length === 0) {
      map.replaceChildren();
      emptyEl.textContent = t("kiosk.noAircraft");
      emptyEl.hidden = false;
      return;
    }

    emptyEl.hidden = true;
    map.replaceChildren(
      ...aircraft.map((row) => {
        const li = document.createElement("li");
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "kiosk-tail-chip";
        btn.dataset.tail = row.tail;
        const isFocused = focusedTail === row.tail;
        btn.setAttribute("aria-pressed", isFocused ? "true" : "false");
        if (isFocused) btn.classList.add("is-focused");
        btn.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
        btn.addEventListener("click", () => {
          toggleFocusTail(row.tail);
        });
        li.appendChild(btn);
        return li;
      }),
    );

    if (activeChipTail) {
      const target = Array.from(
        map.querySelectorAll<HTMLButtonElement>("button[data-tail]"),
      ).find((b) => b.dataset.tail === activeChipTail);
      target?.focus({ preventScroll: true });
    }
  }

  function renderFocusDetail(aircraft: AircraftPosition[]): void {
    const focused = focusedTail
      ? aircraft.find((a) => a.tail === focusedTail) ?? null
      : null;

    if (!focused) {
      focusDetailEl.hidden = true;
      focusDetailEl.replaceChildren();
      return;
    }

    focusDetailEl.setAttribute("aria-label", t("kiosk.focusedAircraftAria"));
    const parts: HTMLElement[] = [];

    const tailEl = document.createElement("strong");
    tailEl.className = "kiosk-focus-tail";
    tailEl.textContent = focused.tail;
    parts.push(tailEl);

    const pilotName =
      typeof focused.pilotName === "string" && focused.pilotName.trim().length > 0
        ? focused.pilotName.trim()
        : null;
    if (pilotName) {
      const pilotEl = document.createElement("span");
      pilotEl.className = "kiosk-focus-item";
      pilotEl.dataset.detail = "pilot";
      pilotEl.textContent = pilotName;
      parts.push(pilotEl);
    }

    const alt = focused.fix?.alt;
    if (typeof alt === "number" && Number.isFinite(alt)) {
      const altEl = document.createElement("span");
      altEl.className = "kiosk-focus-item";
      altEl.dataset.detail = "alt";
      altEl.textContent = `${t("kiosk.detailAltitude")}: ${Math.round(alt)} m`;
      parts.push(altEl);
    }

    const speed = focused.fix?.speed;
    if (typeof speed === "number" && Number.isFinite(speed)) {
      const spdEl = document.createElement("span");
      spdEl.className = "kiosk-focus-item";
      spdEl.dataset.detail = "speed";
      spdEl.textContent = `${t("kiosk.detailSpeed")}: ${Math.round(speed * 3.6)} km/h`;
      parts.push(spdEl);
    }

    focusDetailEl.replaceChildren(...parts);
    focusDetailEl.hidden = false;
  }

  function renderCurrentState(): void {
    renderAircraftList(lastAircraft);
    renderFocusDetail(lastAircraft);
    syncViewAllButton();
    renderTileMap(lastAircraft);
  }

  async function refreshMap(): Promise<void> {
    if (sessionEnded) return;
    const res = await fetch("/api/display/map", { credentials: "same-origin" });
    if (isAuthFailure(res.status)) {
      redirectToLoginOnce();
      return;
    }
    if (!res.ok) return;
    const data = (await res.json()) as {
      aircraft: AircraftPosition[];
    };
    if (sessionEnded) return;

    lastAircraft = data.aircraft ?? [];
    if (focusedTail !== null && !lastAircraft.some((a) => a.tail === focusedTail)) {
      focusedTail = null;
    }

    renderCurrentState();
  }

  function getMapViewportPx(): { w: number; h: number } {
    const rect = (mapContainer ?? mapLayer).getBoundingClientRect();
    const w = rect.width > 0 ? rect.width : mapLayer.clientWidth || viewport.offsetWidth || 1280;
    const h = rect.height > 0 ? rect.height : mapLayer.clientHeight || viewport.offsetHeight || 720;
    return { w, h };
  }

  function getMapPadding(mapPx: { w: number; h: number }): KioskViewPadding {
    const barEl = viewport.querySelector<HTMLElement>("#aircraft-bar") ?? map;
    const barHeight = barEl ? barEl.getBoundingClientRect().height : 0;
    const topSafe = Math.min(Math.round(mapPx.h * 0.45), Math.max(56, Math.ceil(barHeight) + 48));
    const sideSafe = Math.min(Math.round(mapPx.w * 0.2), 56);
    const bottomSafe = Math.min(Math.round(mapPx.h * 0.2), 48);
    return { top: topSafe, right: sideSafe, bottom: bottomSafe, left: sideSafe };
  }

  function renderTileMap(aircraft: AircraftPosition[]): void {
    if (!mapContainer) {
      mapContainer = document.createElement("div");
      mapContainer.id = "kiosk-map";
      mapContainer.setAttribute("data-testid", "kiosk-map");
      mapContainer.style.cssText = `
        position: absolute;
        inset: 0;
        overflow: hidden;
        z-index: 1;
      `;
      mapLayer.prepend(mapContainer);

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
        z-index: 4;
      `;
      attribution.textContent = "Esri";
      mapContainer.appendChild(attribution);
    }

    const existingTiles = mapContainer.querySelectorAll("img[data-tile]");
    existingTiles.forEach((tile) => tile.remove());
    const existingMarkers = mapContainer.querySelectorAll("[data-marker]");
    existingMarkers.forEach((m) => m.remove());

    const mapPx = getMapViewportPx();
    const padding = getMapPadding(mapPx);

    const aircraftPositions = (aircraft ?? [])
      .filter(
        (a): a is AircraftPosition & { fix: { lat: number; lon: number; alt?: number | null; speed?: number | null } } =>
          a.fix != null && Number.isFinite(a.fix.lat) && Number.isFinite(a.fix.lon),
      )
      .map((a) => ({
        lat: a.fix.lat,
        lon: a.fix.lon,
        tail: a.tail,
        pilotName: a.pilotName ?? null,
      }));

    const baseCoords =
      base && base.lat !== null && base.lon !== null
        ? { lat: base.lat, lon: base.lon }
        : null;

    const focusedPos = focusedTail
      ? aircraftPositions.find((a) => a.tail === focusedTail) ?? null
      : null;

    const view = computeKioskView(baseCoords, aircraftPositions, mapPx, {
      focus: focusedPos ? { lat: focusedPos.lat, lon: focusedPos.lon } : null,
      padding,
    });

    mapContainer.setAttribute("data-view", view.viewType);
    if (focusedTail) {
      mapContainer.setAttribute("data-focused-tail", focusedTail);
    } else {
      mapContainer.removeAttribute("data-focused-tail");
    }

    renderTiles(view, mapPx);

    for (const ac of aircraftPositions) {
      renderAircraftMarker(ac, view, mapPx);
    }

    if (base && base.icao && base.lat !== null && base.lon !== null) {
      renderBaseMarker({ lat: base.lat, lon: base.lon, icao: base.icao }, view, mapPx);
    }
  }

  function renderTiles(
    view: ReturnType<typeof computeKioskView>,
    viewportPx: { w: number; h: number },
  ): void {
    const zoom = Math.round(view.zoom);
    const tileSize = 256;

    const existingContainers = mapContainer?.querySelectorAll("[data-tiles]");
    existingContainers?.forEach((c) => c.remove());

    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);

    const scale = Math.pow(2, view.zoom - zoom);
    const halfWidthTiles = viewportPx.w / tileSize / scale / 2;
    const halfHeightTiles = viewportPx.h / tileSize / scale / 2;

    const minX = Math.floor(centerX - halfWidthTiles);
    const maxX = Math.ceil(centerX + halfWidthTiles);
    const minY = Math.floor(centerY - halfHeightTiles);
    const maxY = Math.ceil(centerY + halfHeightTiles);

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

  function renderAircraftMarker(
    ac: { lat: number; lon: number; tail: string; pilotName: string | null },
    view: ReturnType<typeof computeKioskView>,
    mapPx: { w: number; h: number },
  ): void {
    const zoom = view.zoom;
    const tileSize = 256;

    const x = lonToTileX(ac.lon, zoom);
    const y = latToTileY(ac.lat, zoom);

    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);

    const isFocused = focusedTail === ac.tail;
    const marker = document.createElement("button");
    marker.type = "button";
    marker.className = isFocused ? "kiosk-map-aircraft is-focused" : "kiosk-map-aircraft";
    marker.setAttribute("data-marker", "aircraft");
    marker.setAttribute("data-tail", ac.tail);
    marker.setAttribute("aria-pressed", isFocused ? "true" : "false");
    marker.style.left = `${(x - centerX) * tileSize + mapPx.w / 2}px`;
    marker.style.top = `${(y - centerY) * tileSize + mapPx.h / 2}px`;
    marker.style.pointerEvents = "auto";
    marker.style.cursor = "pointer";
    marker.textContent = ac.tail;
    if (ac.pilotName) {
      marker.title = `${ac.tail} · ${ac.pilotName}`;
    }
    marker.addEventListener("click", () => {
      toggleFocusTail(ac.tail);
    });
    mapContainer?.appendChild(marker);
  }

  function renderBaseMarker(
    basePos: { lat: number; lon: number; icao: string },
    view: ReturnType<typeof computeKioskView>,
    mapPx: { w: number; h: number },
  ): void {
    const zoom = view.zoom;
    const tileSize = 256;

    const x = lonToTileX(basePos.lon, zoom);
    const y = latToTileY(basePos.lat, zoom);

    const centerX = lonToTileX(view.centerLon, zoom);
    const centerY = latToTileY(view.centerLat, zoom);

    const marker = document.createElement("div");
    marker.setAttribute("data-marker", "base");
    marker.style.cssText = `
      position: absolute;
      left: ${(x - centerX) * tileSize + mapPx.w / 2}px;
      top: ${(y - centerY) * tileSize + mapPx.h / 2}px;
      transform: translate(-50%, -50%);
      min-width: 28px;
      height: 28px;
      padding: 0 0.35rem;
      background: #22c55e;
      border: 2px solid #fff;
      border-radius: 999px;
      pointer-events: none;
      box-shadow: 0 2px 4px rgba(0,0,0,0.5);
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 0.65rem;
      font-weight: 700;
      color: #052e16;
      z-index: 2;
    `;
    marker.textContent = basePos.icao;
    mapContainer?.appendChild(marker);
  }

  async function fetchLayout(): Promise<void> {
    if (sessionEnded) return;
    const res = await fetch("/api/display/kiosk-layout", { credentials: "same-origin" });
    if (isAuthFailure(res.status)) {
      redirectToLoginOnce();
      return;
    }
    if (!res.ok) return;
    layout = (await res.json()) as KioskQrLayout;
    if (sessionEnded) return;
    if (typeof layout.checkinQrVisible === "boolean") {
      clubQrVisible = layout.checkinQrVisible;
    }
    paintQr();
    if (showQr() && !qr.src) refreshQrImage();
  }

  function watchLayoutStream(): void {
    if (sessionEnded) return;
    if (layoutSource) {
      layoutSource.close();
      layoutSource = null;
    }
    const source = new EventSource("/api/display/kiosk-layout/stream", {
      withCredentials: true,
    });
    layoutSource = source;
    source.addEventListener("layout", (ev) => {
      if (sessionEnded) return;
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
      if (layoutSource === source) layoutSource = null;
      if (sessionEnded) return;
      void fetchLayout().then(() => {
        if (!sessionEnded) {
          window.clearTimeout(sseReconnectTimer);
          sseReconnectTimer = window.setTimeout(watchLayoutStream, 3000);
        }
      });
    };
  }

  async function refetchAll(): Promise<void> {
    if (sessionEnded) return;
    await Promise.all([
      refreshBase().then(() => {
        if (!sessionEnded) return refreshMap();
      }),
      fetchLayout(),
    ]);
    if (!sessionEnded && showQr()) {
      refreshQrImage();
    }
  }

  function scheduleImmediateRefetch(): void {
    if (sessionEnded) return;
    window.clearTimeout(refetchDebounceTimer);
    refetchDebounceTimer = window.setTimeout(() => {
      void refetchAll();
    }, 40);
  }

  async function startActiveLoop(): Promise<void> {
    if (activeLoopStarted || sessionEnded) return;
    activeLoopStarted = true;
    await refreshBase();
    if (sessionEnded) return;
    await fetchLayout();
    if (sessionEnded) return;
    if (localToggleBtn) {
      localToggleBtn.hidden = false;
      syncLocalToggleLabel();
    }
    syncViewAllButton();
    statusEl.textContent = t("kiosk.qrRotateHint", { seconds: String(CHECKIN_STEP_SEC) });
    await refreshMap();
    if (sessionEnded) return;
    watchLayoutStream();
    window.clearInterval(layoutPollTimer);
    layoutPollTimer = window.setInterval(() => void fetchLayout(), 8000);
    startQrRefreshLoop();
    window.clearInterval(mapPollTimer);
    mapPollTimer = window.setInterval(() => void refreshMap(), 5000);
  }

  window.addEventListener("resize", () => {
    if (sessionEnded) return;
    paintQr();
    if (mapContainer) {
      renderTileMap(lastAircraft);
    }
  });

  localToggleBtn?.addEventListener("click", () => {
    if (sessionEnded) return;
    localQrHidden = !localQrHidden;
    syncLocalToggleLabel();
    paintQr();
    if (showQr()) refreshQrImage();
  });

  viewAllBtn?.addEventListener("click", () => {
    if (sessionEnded) return;
    clearFocus();
  });

  unsubscribeSignals = subscribeKioskSignals((signal) => {
    if (signal === "logout") {
      redirectToLoginOnce();
      return;
    }
    if (signal === "refetch") {
      scheduleImmediateRefetch();
    }
  });

  void (async () => {
    await startClubPage();
    if (sessionEnded) return;
    await startActiveLoop();
  })();
}
