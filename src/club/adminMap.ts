import "./adminMap.css";
import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { notifyKioskRefetch } from "./kioskSync";
import {
  ALLOWED_TRAIL_MINUTES,
  DEFAULT_TRAIL_MINUTES,
  parseTrailMinutes,
  parseTrailsPayload,
  trailOptionI18n,
  type FlightTrail,
  type TrailMinutes,
} from "./liveTrail";
import { mountOpenKioskLink, type OpenKioskLinkController } from "./openKioskLink";
import { safeNextPath } from "./safeNext";
import {
  aircraftSourceBadgeLabel,
  extractPublicAircraft,
  formatAircraftListLabel,
  normalizeBaseAerodrome,
  renderClubTileMap,
  type NormalizedBase,
  type PublicMapAircraft,
} from "./tileMapRenderer";
import { bundledLocaleIds, localeLabel, t } from "../i18n/index";

export function canConfirmClubDelete(typed: string, liveSlug: string | null): boolean {
  if (!liveSlug) return false;
  return typed.trim() === liveSlug;
}

export type DeleteClubController = {
  configure: (opts: { isAdmin: boolean; clubSlug: string | null }) => void;
  updateSlug: (slug: string) => void;
};

export function mountDeleteClubSection(
  navigate: (url: string) => void = (url) => window.location.assign(url),
): DeleteClubController {
  const section = document.querySelector<HTMLElement>("#club-danger-zone");
  const expectedEl = document.querySelector<HTMLElement>("#club-delete-slug-expected");
  const input = document.querySelector<HTMLInputElement>("#club-delete-slug-input");
  const confirmBtn = document.querySelector<HTMLButtonElement>("#club-delete-confirm");
  const msgEl = document.querySelector<HTMLParagraphElement>("#club-delete-msg");

  let liveClubSlug: string | null = null;
  let isAdmin = false;
  let submitting = false;

  function syncState(): void {
    if (section) section.hidden = !isAdmin;
    if (expectedEl) expectedEl.textContent = liveClubSlug ?? "";
    if (input) input.disabled = submitting;
    if (confirmBtn) {
      confirmBtn.disabled =
        submitting || !isAdmin || !canConfirmClubDelete(input?.value ?? "", liveClubSlug);
    }
  }

  input?.addEventListener("input", () => {
    if (msgEl) msgEl.textContent = "";
    syncState();
  });

  confirmBtn?.addEventListener("click", async () => {
    if (!input || !isAdmin || submitting) return;
    const slug = input.value.trim();
    if (!canConfirmClubDelete(slug, liveClubSlug)) {
      if (msgEl) msgEl.textContent = t("adminMap.deleteClubWrongSlug");
      syncState();
      return;
    }
    submitting = true;
    if (msgEl) msgEl.textContent = "";
    syncState();
    let navigated = false;
    try {
      const res = await fetch("/api/admin/club", {
        method: "DELETE",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ slug }),
      });
      const payload = (await res.json().catch(() => ({}))) as {
        ok?: boolean;
        redirect?: string;
        error?: string;
      };
      if (res.ok) {
        const dest =
          typeof payload.redirect === "string" && payload.redirect.trim()
            ? safeNextPath(payload.redirect)
            : "/account-clubs.html";
        navigated = true;
        navigate(dest);
        return;
      }
      if (msgEl) {
        const err = payload.error ?? "";
        if (err === "slug mismatch" || err === "invalid slug") {
          msgEl.textContent = t("adminMap.deleteClubWrongSlug");
        } else if (err === "admin only") {
          msgEl.textContent = t("api.adminOnly");
        } else {
          msgEl.textContent = t("adminMap.deleteClubFailed");
        }
      }
    } catch {
      if (msgEl) msgEl.textContent = t("adminMap.deleteClubFailed");
    } finally {
      if (!navigated) {
        submitting = false;
        syncState();
      }
    }
  });

  syncState();

  return {
    configure(opts) {
      isAdmin = opts.isAdmin;
      liveClubSlug = opts.clubSlug && opts.clubSlug.length > 0 ? opts.clubSlug : null;
      syncState();
    },
    updateSlug(slug) {
      liveClubSlug = slug.length > 0 ? slug : null;
      syncState();
    },
  };
}

export type ClubTrailSettingController = {
  configure: (opts: { isAdmin: boolean; trailMinutes: TrailMinutes }) => void;
  relocalize: () => void;
};

export function mountClubTrailSetting(
  onSaved: () => void = () => {},
): ClubTrailSettingController {
  const chipsContainer = document.querySelector<HTMLDivElement>("#club-trail-chips");
  const msgEl = document.querySelector<HTMLParagraphElement>("#club-trail-msg");

  let isAdmin = false;
  let currentValue: TrailMinutes = DEFAULT_TRAIL_MINUTES;
  let saving = false;

  function chipLabel(value: TrailMinutes): string {
    const { key, params } = trailOptionI18n(value);
    return t(key, params);
  }

  function renderChips(): void {
    if (!chipsContainer) return;
    chipsContainer.setAttribute("aria-label", t("admin.trailTitle"));
    chipsContainer.replaceChildren(
      ...ALLOWED_TRAIL_MINUTES.map((value) => {
        const btn = document.createElement("button");
        btn.type = "button";
        const isSelected = value === currentValue;
        btn.className = isSelected ? "chip trail-chip active" : "chip trail-chip";
        btn.dataset.trailValue = String(value);
        btn.setAttribute("aria-pressed", isSelected ? "true" : "false");
        btn.disabled = saving || !isAdmin;
        btn.textContent = chipLabel(value);
        btn.addEventListener("click", () => {
          void selectValue(value);
        });
        return btn;
      }),
    );
  }

  async function selectValue(nextValue: TrailMinutes): Promise<void> {
    if (!isAdmin || saving || nextValue === currentValue) return;
    const previous = currentValue;
    currentValue = nextValue;
    saving = true;
    if (msgEl) {
      msgEl.setAttribute("role", "status");
      msgEl.textContent = t("admin.saving");
    }
    renderChips();
    try {
      const res = await fetch("/api/admin/club/trail", {
        method: "PATCH",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ trailMinutes: nextValue }),
      });
      const payload = (await res.json().catch(() => ({}))) as {
        trailMinutes?: unknown;
        error?: string;
      };
      if (res.ok) {
        currentValue = parseTrailMinutes(payload.trailMinutes, nextValue);
        if (msgEl) {
          msgEl.setAttribute("role", "status");
          msgEl.textContent = t("admin.trailSaved");
        }
        onSaved();
      } else {
        currentValue = previous;
        if (msgEl) {
          msgEl.setAttribute("role", "alert");
          msgEl.textContent =
            payload.error === "admin only" ? t("api.adminOnly") : t("api.dbError");
        }
      }
    } catch {
      currentValue = previous;
      if (msgEl) {
        msgEl.setAttribute("role", "alert");
        msgEl.textContent = t("api.dbError");
      }
    } finally {
      saving = false;
      renderChips();
    }
  }

  renderChips();

  return {
    configure(opts) {
      isAdmin = opts.isAdmin;
      currentValue = parseTrailMinutes(opts.trailMinutes, DEFAULT_TRAIL_MINUTES);
      renderChips();
    },
    relocalize() {
      renderChips();
    },
  };
}

export const ALLOWED_NEARBY_PUBLIC_RADII = [10, 25, 50, 100, 200] as const;
export const DEFAULT_NEARBY_PUBLIC_RADIUS_KM = 50;

export type ClubNearbyPublicController = {
  configure: (opts: {
    isAdmin: boolean;
    showNearbyPublic: boolean;
    nearbyPublicRadiusKm: number;
  }) => void;
  relocalize: () => void;
};

export function mountClubNearbyPublicSetting(
  onSaved: () => void = () => {},
): ClubNearbyPublicController {
  const toggleEl = document.querySelector<HTMLInputElement>("#club-nearby-public-toggle");
  const radiusEl = document.querySelector<HTMLSelectElement>("#club-nearby-public-radius");
  const msgEl = document.querySelector<HTMLParagraphElement>("#club-nearby-public-msg");

  let isAdmin = false;
  let showNearbyPublic = true;
  let radiusKm = DEFAULT_NEARBY_PUBLIC_RADIUS_KM;
  let saving = false;

  function renderRadiusOptions(): void {
    if (!radiusEl) return;
    const values: number[] = [...ALLOWED_NEARBY_PUBLIC_RADII];
    if (!values.includes(radiusKm)) {
      values.push(radiusKm);
      values.sort((a, b) => a - b);
    }
    radiusEl.replaceChildren(
      ...values.map((km) => {
        const opt = document.createElement("option");
        opt.value = String(km);
        opt.textContent = t("admin.nearbyPublicRadiusOption", { km: String(km) });
        opt.selected = km === radiusKm;
        return opt;
      }),
    );
  }

  function syncControls(): void {
    if (toggleEl) {
      toggleEl.checked = showNearbyPublic;
      toggleEl.disabled = saving || !isAdmin;
    }
    if (radiusEl) {
      radiusEl.value = String(radiusKm);
      radiusEl.disabled = saving || !isAdmin || !showNearbyPublic;
    }
  }

  async function persist(nextShow: boolean, nextRadius: number): Promise<void> {
    if (!isAdmin || saving) return;
    const prevShow = showNearbyPublic;
    const prevRadius = radiusKm;
    showNearbyPublic = nextShow;
    radiusKm = nextRadius;
    saving = true;
    if (msgEl) {
      msgEl.setAttribute("role", "status");
      msgEl.textContent = t("admin.saving");
    }
    syncControls();
    try {
      const res = await fetch("/api/admin/club/nearby-public", {
        method: "PATCH",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          showNearbyPublic: nextShow,
          nearbyPublicRadiusKm: nextRadius,
        }),
      });
      const payload = (await res.json().catch(() => ({}))) as {
        showNearbyPublic?: unknown;
        nearbyPublicRadiusKm?: unknown;
        error?: string;
      };
      if (res.ok) {
        if (typeof payload.showNearbyPublic === "boolean") {
          showNearbyPublic = payload.showNearbyPublic;
        }
        if (
          typeof payload.nearbyPublicRadiusKm === "number" &&
          Number.isInteger(payload.nearbyPublicRadiusKm)
        ) {
          radiusKm = payload.nearbyPublicRadiusKm;
        }
        if (msgEl) {
          msgEl.setAttribute("role", "status");
          msgEl.textContent = t("admin.nearbyPublicSaved");
        }
        onSaved();
      } else {
        showNearbyPublic = prevShow;
        radiusKm = prevRadius;
        if (msgEl) {
          msgEl.setAttribute("role", "alert");
          msgEl.textContent =
            payload.error === "admin only" ? t("api.adminOnly") : t("api.dbError");
        }
      }
    } catch {
      showNearbyPublic = prevShow;
      radiusKm = prevRadius;
      if (msgEl) {
        msgEl.setAttribute("role", "alert");
        msgEl.textContent = t("api.dbError");
      }
    } finally {
      saving = false;
      renderRadiusOptions();
      syncControls();
    }
  }

  toggleEl?.addEventListener("change", () => {
    void persist(toggleEl.checked, radiusKm);
  });

  radiusEl?.addEventListener("change", () => {
    const parsed = Number(radiusEl.value);
    if (!Number.isInteger(parsed) || parsed < 5 || parsed > 500) return;
    void persist(showNearbyPublic, parsed);
  });

  renderRadiusOptions();
  syncControls();

  return {
    configure(opts) {
      isAdmin = opts.isAdmin;
      showNearbyPublic = opts.showNearbyPublic;
      radiusKm =
        Number.isInteger(opts.nearbyPublicRadiusKm) &&
        opts.nearbyPublicRadiusKm >= 5 &&
        opts.nearbyPublicRadiusKm <= 500
          ? opts.nearbyPublicRadiusKm
          : DEFAULT_NEARBY_PUBLIC_RADIUS_KM;
      renderRadiusOptions();
      syncControls();
    },
    relocalize() {
      renderRadiusOptions();
      syncControls();
    },
  };
}

function bootAdminMap(): void {
  const mapElMaybe = document.querySelector<HTMLDivElement>("#admin-map");
  const baseBadgeEl = document.querySelector<HTMLSpanElement>("#admin-base-badge");
  const liveEmpty = document.querySelector<HTMLParagraphElement>("#live-empty");
  const listMaybe = document.querySelector<HTMLUListElement>("#live");
  const ticksMaybe = document.querySelector<HTMLDivElement>("#ticks");
  const tailsAllBtn = document.querySelector<HTMLButtonElement>("#btn-tails-all");
  const tailsNoneBtn = document.querySelector<HTMLButtonElement>("#btn-tails-none");
  const saveMaybe = document.querySelector<HTMLButtonElement>("#save");
  const tailsMsg = document.querySelector<HTMLParagraphElement>("#tails-msg");
  const openKioskSlot = document.querySelector<HTMLDivElement>("#open-kiosk-slot");
  const clubLocaleSelect = document.querySelector<HTMLSelectElement>("#club-locale-select");
  const clubLocaleHelp = document.querySelector<HTMLParagraphElement>("#club-locale-help");
  const clubLocaleSave = document.querySelector<HTMLButtonElement>("#club-locale-save");
  const clubLocaleMsg = document.querySelector<HTMLParagraphElement>("#club-locale-msg");
  const clubSlugInput = document.querySelector<HTMLInputElement>("#club-slug-input");
  const clubSlugHelp = document.querySelector<HTMLParagraphElement>("#club-slug-help");
  const clubSlugSave = document.querySelector<HTMLButtonElement>("#club-slug-save");
  const clubSlugMsg = document.querySelector<HTMLParagraphElement>("#club-slug-msg");
  const clubBaseIcaoInput = document.querySelector<HTMLInputElement>("#club-base-icao-input");
  const clubBaseLatInput = document.querySelector<HTMLInputElement>("#club-base-lat-input");
  const clubBaseLonInput = document.querySelector<HTMLInputElement>("#club-base-lon-input");
  const clubBaseSave = document.querySelector<HTMLButtonElement>("#club-base-save");
  const clubBaseMsg = document.querySelector<HTMLParagraphElement>("#club-base-msg");

  if (!mapElMaybe || !listMaybe || !ticksMaybe || !saveMaybe) {
    return;
  }
  const mapEl: HTMLDivElement = mapElMaybe;
  const list: HTMLUListElement = listMaybe;
  const ticks: HTMLDivElement = ticksMaybe;
  const save: HTMLButtonElement = saveMaybe;

  let currentClubSlug = "cas-tobalaba";
  let currentBase: NormalizedBase | null = null;
  let lastLiveAircraft: PublicMapAircraft[] = [];
  let lastLiveTrails: FlightTrail[] = [];
  let kioskLinkCtrl: OpenKioskLinkController | null = null;
  const deleteClubCtrl = mountDeleteClubSection();
  const trailCtrl = mountClubTrailSetting(() => {
    notifyKioskRefetch();
    void refreshLiveMap();
  });
  const nearbyPublicCtrl = mountClubNearbyPublicSetting(() => {
    notifyKioskRefetch();
    void refreshLiveMap();
  });

  function paintSlugHelp(): void {
    const slug = currentClubSlug || "cas-tobalaba";
    if (clubLocaleHelp) {
      clubLocaleHelp.textContent = t("admin.clubDefaultLanguageHelp", { slug });
    }
    if (clubSlugHelp) {
      clubSlugHelp.textContent = t("admin.clubSlugHelp", { slug });
    }
  }

  function paintBaseBadge(): void {
    if (!baseBadgeEl) return;
    if (currentBase?.icao) {
      baseBadgeEl.textContent = currentBase.icao;
      baseBadgeEl.hidden = false;
    } else {
      baseBadgeEl.hidden = true;
    }
  }

  function paintLiveMap(): void {
    renderClubTileMap(mapEl, {
      base: currentBase,
      aircraft: lastLiveAircraft,
      trails: lastLiveTrails,
      trailLineWidthPx: 3,
    });
  }

  function paintLiveList(): void {
    if (lastLiveAircraft.length === 0) {
      list.replaceChildren();
      if (liveEmpty) {
        liveEmpty.textContent = t("adminMap.noLiveAircraft");
        liveEmpty.hidden = false;
      }
      return;
    }
    if (liveEmpty) liveEmpty.hidden = true;
    list.replaceChildren(
      ...lastLiveAircraft.map((row) => {
        const li = document.createElement("li");
        if (row.tailSource) {
          li.setAttribute("data-tail-source", row.tailSource);
        }
        const badgeLabel = aircraftSourceBadgeLabel(row);
        if (badgeLabel) {
          li.className =
            row.tailSource === "pilot" ? "is-non-club is-pilot-tail" : "is-non-club";
          li.setAttribute("data-non-club", "true");
          if (row.otherClub === true) {
            li.setAttribute("data-other-club", "true");
          }
          li.textContent = formatAircraftListLabel(row);
        } else {
          li.textContent = formatAircraftListLabel(row, { fallbackPilotDash: true });
        }
        return li;
      }),
    );
  }

  /** Loads tail filter checkboxes on page load and after Save only (never from the 5s poll). */
  async function loadTails(): Promise<void> {
    const [aircraft, selected] = await Promise.all([
      api<{ aircraft: { tail: string }[] }>("/api/admin/aircraft"),
      api<{ tails: string[] }>("/api/admin/map/tails"),
    ]);
    const set = new Set(selected.tails);
    if (aircraft.aircraft.length === 0) {
      const empty = document.createElement("p");
      empty.className = "muted";
      empty.textContent = t("adminMap.noTailsRegistered");
      ticks.replaceChildren(empty);
      return;
    }
    ticks.replaceChildren(
      ...aircraft.aircraft.map((a) => {
        const label = document.createElement("label");
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.value = a.tail;
        cb.checked = set.size === 0 || set.has(a.tail);
        label.append(cb, document.createTextNode(` ${a.tail}`));
        return label;
      }),
    );
  }

  /** Polled every 5 seconds: refreshes live aircraft list, 2D map markers, and live trails. */
  async function refreshLiveMap(): Promise<void> {
    const [live, trailsRes] = await Promise.all([
      api<{ aircraft: unknown }>("/api/admin/map"),
      fetch("/api/admin/map/trails", { credentials: "same-origin" }).catch(() => null),
    ]);
    lastLiveAircraft = extractPublicAircraft(live.aircraft);
    lastLiveTrails =
      trailsRes && trailsRes.ok
        ? parseTrailsPayload(await trailsRes.json().catch(() => null))
        : [];
    paintLiveMap();
    paintLiveList();
  }

  function mountClubLocalePicker(current?: string): void {
    if (!clubLocaleSelect) return;
    clubLocaleSelect.replaceChildren(
      ...bundledLocaleIds().map((id) => {
        const opt = document.createElement("option");
        opt.value = id;
        opt.textContent = localeLabel(id);
        opt.selected = id === (current ?? "es");
        return opt;
      }),
    );
  }

  function clearTailsMessage(): void {
    if (tailsMsg) tailsMsg.textContent = "";
  }

  ticks.addEventListener("change", clearTailsMessage);

  tailsAllBtn?.addEventListener("click", () => {
    for (const cb of ticks.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')) {
      cb.checked = true;
    }
    clearTailsMessage();
  });

  tailsNoneBtn?.addEventListener("click", () => {
    for (const cb of ticks.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')) {
      cb.checked = false;
    }
    clearTailsMessage();
  });

  save.addEventListener("click", async () => {
    save.disabled = true;
    try {
      const tails = [...ticks.querySelectorAll<HTMLInputElement>("input:checked")].map((i) => i.value);
      await api("/api/admin/map/tails", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ tails }),
      });
      notifyKioskRefetch();
      await Promise.all([loadTails(), refreshLiveMap()]);
      if (tailsMsg) {
        tailsMsg.setAttribute("role", "status");
        tailsMsg.textContent = t("adminMap.selectionSaved");
      }
    } catch (e) {
      console.error("[adminMap] save tails failed:", e);
      if (tailsMsg) {
        tailsMsg.setAttribute("role", "alert");
        tailsMsg.textContent = t("api.genericError");
      }
    } finally {
      save.disabled = false;
    }
  });

  function slugApiMessage(res: Response, payload: { error?: string }): string {
    if (res.ok) return t("admin.clubSlugSaved");
    const err = payload.error ?? "";
    if (err === "invalid slug") return t("api.invalidSlug");
    if (err === "slug taken") return t("api.slugTaken");
    if (err === "unchanged") return t("api.unchanged");
    return t("api.dbError");
  }

  clubSlugSave?.addEventListener("click", async () => {
    if (!clubSlugInput) return;
    const slug = clubSlugInput.value.trim().toLowerCase();
    const res = await fetch("/api/admin/club/slug", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ slug }),
    });
    const payload = (await res.json().catch(() => ({}))) as { error?: string; slug?: string };
    if (res.ok && payload.slug) {
      clubSlugInput.value = payload.slug;
      currentClubSlug = payload.slug;
      paintSlugHelp();
      kioskLinkCtrl?.updateSlug(payload.slug);
      deleteClubCtrl.updateSlug(payload.slug);
      notifyKioskRefetch();
    }
    if (clubSlugMsg) clubSlugMsg.textContent = slugApiMessage(res, payload);
  });

  clubLocaleSave?.addEventListener("click", async () => {
    if (!clubLocaleSelect) return;
    const res = await fetch("/api/admin/club/locale", {
      method: "PATCH",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ locale: clubLocaleSelect.value }),
    });
    if (clubLocaleMsg) {
      clubLocaleMsg.textContent = res.ok ? t("admin.clubDefaultLanguageSaved") : t("api.dbError");
    }
  });

  clubBaseSave?.addEventListener("click", async () => {
    if (!clubBaseIcaoInput || !clubBaseLatInput || !clubBaseLonInput) return;

    const icao = clubBaseIcaoInput.value.trim();
    const lat = parseFloat(clubBaseLatInput.value.trim());
    const lon = parseFloat(clubBaseLonInput.value.trim());

    const res = await fetch("/api/admin/club/base", {
      method: "PATCH",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ icao, lat, lon }),
    });

    if (res.ok) {
      const data = (await res.json()) as { icao: string | null; lat: number | null; lon: number | null };
      if (clubBaseMsg) {
        clubBaseMsg.textContent = t("admin.baseSaved");
      }
      if (data.icao) clubBaseIcaoInput.value = data.icao;
      if (data.lat !== null) clubBaseLatInput.value = String(data.lat);
      if (data.lon !== null) clubBaseLonInput.value = String(data.lon);
      currentBase = normalizeBaseAerodrome(data);
      paintBaseBadge();
      paintLiveMap();
      notifyKioskRefetch();
    } else {
      const payload = (await res.json().catch(() => ({}))) as { error?: string };
      if (clubBaseMsg) {
        clubBaseMsg.textContent = payload.error ? t("admin.invalidBase") : t("api.dbError");
      }
    }
  });

  window.addEventListener("resize", () => {
    paintLiveMap();
  });

  window.addEventListener("fv-locale-change", () => {
    paintSlugHelp();
    paintLiveMap();
    paintLiveList();
    trailCtrl.relocalize();
    nearbyPublicCtrl.relocalize();
  });

  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    try {
      const me = await api<{
        pilot?: { role?: string } | null;
        email_verified?: boolean;
        clubDefaultLocale?: string;
        clubTrailMinutesDefault?: number;
        clubShowNearbyPublic?: boolean;
        clubNearbyPublicRadiusKm?: number;
        clubSlug?: string;
      }>("/api/club/me");
      mountClubLocalePicker(me.clubDefaultLocale ?? "es");
      if (me.clubSlug) currentClubSlug = me.clubSlug;
      if (clubSlugInput) clubSlugInput.value = me.clubSlug ?? "";
      paintSlugHelp();
      const isAdmin = me.pilot?.role === "admin";
      deleteClubCtrl.configure({
        isAdmin,
        clubSlug: me.clubSlug ?? null,
      });
      trailCtrl.configure({
        isAdmin,
        trailMinutes: parseTrailMinutes(me.clubTrailMinutesDefault, DEFAULT_TRAIL_MINUTES),
      });
      nearbyPublicCtrl.configure({
        isAdmin,
        showNearbyPublic: me.clubShowNearbyPublic ?? true,
        nearbyPublicRadiusKm:
          typeof me.clubNearbyPublicRadiusKm === "number"
            ? me.clubNearbyPublicRadiusKm
            : DEFAULT_NEARBY_PUBLIC_RADIUS_KM,
      });
      kioskLinkCtrl = await mountOpenKioskLink(openKioskSlot, { clubSlug: me.clubSlug ?? null });

      const baseRes = await api<{ icao: string | null; lat: number | null; lon: number | null }>(
        "/api/admin/club/base",
      );
      if (clubBaseIcaoInput) clubBaseIcaoInput.value = baseRes.icao ?? "";
      if (clubBaseLatInput) clubBaseLatInput.value = baseRes.lat !== null ? String(baseRes.lat) : "";
      if (clubBaseLonInput) clubBaseLonInput.value = baseRes.lon !== null ? String(baseRes.lon) : "";
      currentBase = normalizeBaseAerodrome(baseRes);
      paintBaseBadge();

      await requireAdmin();
      await Promise.all([loadTails(), refreshLiveMap()]);
      window.setInterval(() => void refreshLiveMap(), 5000);
    } catch (e) {
      if (!isClubAdminRedirect(e)) {
        const li = document.createElement("li");
        li.textContent = t("admin.adminRequired");
        list.replaceChildren(li);
      }
    }
  })();
}

if (typeof document !== "undefined" && document.querySelector("#admin-map")) {
  bootAdminMap();
}
