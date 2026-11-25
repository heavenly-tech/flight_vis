import { startClubPage } from "./i18nPage";
import { apiErrorMessage, t } from "../i18n/index";
import {
  parsePublicMapPayload,
  renderClubTileMap,
  type PublicMapPayload,
} from "./tileMapRenderer";

const POLL_INTERVAL_MS = 5000;

function requireEl(id: string): HTMLElement {
  const el = document.getElementById(id);
  if (!el) throw new Error(`public-map element missing: #${id}`);
  return el;
}

const mapEl = requireEl("public-map");
const listEl = requireEl("list");
const clubNameEl = requireEl("club-name");
const baseBadgeEl = document.getElementById("base-badge");
const emptyStateEl = requireEl("empty-state");
const errorEl = requireEl("map-error");

const clubParam = new URLSearchParams(location.search).get("club");
const mapUrl = clubParam
  ? `/api/club/map/public?club=${encodeURIComponent(clubParam)}`
  : "/api/club/map/public";

let lastPayload: PublicMapPayload | null = null;
let lastErrorKey: string | null = null;

function paintState(payload: PublicMapPayload): void {
  const pageTitle = t("club.publicMap.title");
  delete clubNameEl.dataset.i18n;
  if (payload.clubName) {
    clubNameEl.textContent = payload.clubName;
    document.title = `${payload.clubName} · ${pageTitle}`;
  } else {
    clubNameEl.textContent = pageTitle;
    document.title = pageTitle;
  }

  if (baseBadgeEl) {
    if (payload.base?.icao) {
      baseBadgeEl.textContent = payload.base.icao;
      baseBadgeEl.hidden = false;
    } else {
      baseBadgeEl.hidden = true;
    }
  }

  errorEl.hidden = true;
  renderClubTileMap(mapEl, {
    base: payload.base,
    aircraft: payload.aircraft,
  });

  if (payload.aircraft.length === 0) {
    listEl.replaceChildren();
    emptyStateEl.textContent = t("club.publicMap.empty");
    emptyStateEl.hidden = false;
    return;
  }

  emptyStateEl.hidden = true;
  listEl.replaceChildren(
    ...payload.aircraft.map((row) => {
      const li = document.createElement("li");
      const name = row.pilotName ? ` · ${row.pilotName}` : "";
      li.textContent = `${row.tail}${name}`;
      return li;
    }),
  );
}

function paintError(rawError: string | null): void {
  lastPayload = null;
  lastErrorKey = rawError;
  clubNameEl.textContent = t("club.publicMap.title");
  if (baseBadgeEl) baseBadgeEl.hidden = true;
  emptyStateEl.hidden = true;
  listEl.replaceChildren();
  errorEl.textContent = rawError ? apiErrorMessage(rawError) : t("club.publicMap.unavailable");
  errorEl.hidden = false;
  renderClubTileMap(mapEl, { base: null, aircraft: [] });
}

async function fetchMapData(): Promise<
  { ok: true; payload: PublicMapPayload } | { ok: false; error: string | null }
> {
  try {
    const res = await fetch(mapUrl, { credentials: "same-origin" });
    if (!res.ok) {
      const errBody = (await res.json().catch(() => ({}))) as { error?: unknown };
      const errStr = typeof errBody.error === "string" ? errBody.error : null;
      return { ok: false, error: errStr };
    }
    const raw = (await res.json()) as unknown;
    return { ok: true, payload: parsePublicMapPayload(raw) };
  } catch {
    return { ok: false, error: null };
  }
}

async function refresh(): Promise<void> {
  const result = await fetchMapData();
  if (result.ok) {
    lastPayload = result.payload;
    lastErrorKey = null;
    paintState(result.payload);
  } else if (!lastPayload) {
    paintError(result.error);
  }
}

// Kick off i18n and initial map fetch in parallel so club name + map show immediately.
const [_, initial] = await Promise.all([
  startClubPage({ persistUser: false }),
  fetchMapData(),
]);

if (initial.ok) {
  lastPayload = initial.payload;
  paintState(initial.payload);
} else {
  paintError(initial.error);
}

window.addEventListener("resize", () => {
  if (lastPayload) {
    renderClubTileMap(mapEl, {
      base: lastPayload.base,
      aircraft: lastPayload.aircraft,
    });
  } else {
    renderClubTileMap(mapEl, { base: null, aircraft: [] });
  }
});

window.addEventListener("fv-locale-change", () => {
  if (lastPayload) {
    paintState(lastPayload);
  } else if (lastErrorKey !== null || !errorEl.hidden) {
    paintError(lastErrorKey);
  }
});

window.setInterval(() => {
  void refresh();
}, POLL_INTERVAL_MS);
