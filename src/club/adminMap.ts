import "./adminMap.css";
import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { notifyKioskRefetch } from "./kioskSync";
import { mountOpenKioskLink, type OpenKioskLinkController } from "./openKioskLink";
import {
  extractPublicAircraft,
  normalizeBaseAerodrome,
  renderClubTileMap,
  type NormalizedBase,
  type PublicMapAircraft,
} from "./tileMapRenderer";
import { bundledLocaleIds, localeLabel, t } from "../i18n/index";

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
  let kioskLinkCtrl: OpenKioskLinkController | null = null;

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
        li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
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

  /** Polled every 5 seconds: refreshes live aircraft list and 2D map markers only. */
  async function refreshLiveMap(): Promise<void> {
    const live = await api<{ aircraft: unknown }>("/api/admin/map");
    lastLiveAircraft = extractPublicAircraft(live.aircraft);
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
    paintLiveList();
  });

  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    try {
      const me = await api<{ clubDefaultLocale?: string; clubSlug?: string }>("/api/club/me");
      mountClubLocalePicker(me.clubDefaultLocale ?? "es");
      if (me.clubSlug) currentClubSlug = me.clubSlug;
      if (clubSlugInput) clubSlugInput.value = me.clubSlug ?? "";
      paintSlugHelp();
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
