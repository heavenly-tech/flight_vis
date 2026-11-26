import "./adminMap.css";
import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { mountOpenKioskLink, type OpenKioskLinkController } from "./openKioskLink";
import { safeNextPath } from "./safeNext";
import {
  extractPublicAircraft,
  normalizeBaseAerodrome,
  renderClubTileMap,
  type NormalizedBase,
  type PublicMapAircraft,
} from "./tileMapRenderer";
import { bundledLocaleIds, localeLabel, t } from "../i18n/index";

export type CopyResult = "copied" | "selected";

export async function copyTextWithFallback(
  text: string,
  input: HTMLInputElement,
): Promise<CopyResult> {
  if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
    try {
      await navigator.clipboard.writeText(text);
      return "copied";
    } catch {
      // Fall through to input selection + execCommand fallback.
    }
  }
  input.focus();
  input.select();
  if (typeof input.setSelectionRange === "function") {
    input.setSelectionRange(0, text.length);
  }
  try {
    if (typeof document.execCommand === "function" && document.execCommand("copy")) {
      return "copied";
    }
  } catch {
    // Selection-only fallback below.
  }
  return "selected";
}

type DisplaySessionInfo = {
  fullUrl: string;
  token: string;
};

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
  const kioskMaybe = document.querySelector<HTMLButtonElement>("#btn-kiosk");
  const kioskOutMaybe = document.querySelector<HTMLDivElement>("#kiosk-url");
  const kioskCopyStatus = document.querySelector<HTMLParagraphElement>("#kiosk-copy-status");
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

  if (!mapElMaybe || !listMaybe || !ticksMaybe || !saveMaybe || !kioskMaybe || !kioskOutMaybe) {
    return;
  }
  const mapEl: HTMLDivElement = mapElMaybe;
  const list: HTMLUListElement = listMaybe;
  const ticks: HTMLDivElement = ticksMaybe;
  const save: HTMLButtonElement = saveMaybe;
  const kiosk: HTMLButtonElement = kioskMaybe;
  const kioskOut: HTMLDivElement = kioskOutMaybe;

  let currentClubSlug = "cas-tobalaba";
  let currentBase: NormalizedBase | null = null;
  let lastLiveAircraft: PublicMapAircraft[] = [];
  let lastDisplaySession: DisplaySessionInfo | null = null;
  let kioskLinkCtrl: OpenKioskLinkController | null = null;
  const deleteClubCtrl = mountDeleteClubSection();

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

  function setCopyFeedback(result: CopyResult): void {
    if (!kioskCopyStatus) return;
    kioskCopyStatus.setAttribute("role", "status");
    kioskCopyStatus.textContent =
      result === "copied" ? t("common.copied") : t("adminMap.copySelected");
  }

  function renderDisplaySession(info: DisplaySessionInfo): void {
    const urlGroup = document.createElement("div");
    urlGroup.className = "kiosk-copy-group";

    const urlLabel = document.createElement("label");
    urlLabel.className = "kiosk-copy-label";
    urlLabel.htmlFor = "kiosk-display-url";
    urlLabel.textContent = t("admin.kioskOpenUrl");

    const urlRow = document.createElement("div");
    urlRow.className = "kiosk-copy-row";

    const urlInput = document.createElement("input");
    urlInput.id = "kiosk-display-url";
    urlInput.type = "text";
    urlInput.readOnly = true;
    urlInput.className = "kiosk-copy-input";
    urlInput.value = info.fullUrl;
    urlInput.setAttribute("aria-label", t("adminMap.displayUrlLabel"));

    const copyUrlBtn = document.createElement("button");
    copyUrlBtn.type = "button";
    copyUrlBtn.id = "btn-copy-kiosk-url";
    copyUrlBtn.className = "btn-secondary";
    copyUrlBtn.textContent = t("adminMap.copyLink");
    copyUrlBtn.addEventListener("click", async () => {
      const result = await copyTextWithFallback(info.fullUrl, urlInput);
      setCopyFeedback(result);
    });

    const openLink = document.createElement("a");
    openLink.id = "kiosk-open-session";
    openLink.className = "btn-secondary";
    openLink.href = info.fullUrl;
    openLink.target = "_blank";
    openLink.rel = "noopener";
    openLink.textContent = t("adminMap.openDisplay");

    urlRow.append(urlInput, copyUrlBtn, openLink);
    urlGroup.append(urlLabel, urlRow);

    const codeGroup = document.createElement("div");
    codeGroup.className = "kiosk-copy-group";

    const codeLabel = document.createElement("label");
    codeLabel.className = "kiosk-copy-label";
    codeLabel.htmlFor = "kiosk-display-token";
    codeLabel.textContent = t("admin.kioskActivationToken");

    const codeRow = document.createElement("div");
    codeRow.className = "kiosk-copy-row";

    const codeInput = document.createElement("input");
    codeInput.id = "kiosk-display-token";
    codeInput.type = "text";
    codeInput.readOnly = true;
    codeInput.className = "kiosk-copy-input";
    codeInput.value = info.token;
    codeInput.setAttribute("aria-label", t("adminMap.activationCodeLabel"));

    const copyCodeBtn = document.createElement("button");
    copyCodeBtn.type = "button";
    copyCodeBtn.id = "btn-copy-kiosk-code";
    copyCodeBtn.className = "btn-secondary";
    copyCodeBtn.textContent = t("adminMap.copyCode");
    copyCodeBtn.addEventListener("click", async () => {
      const result = await copyTextWithFallback(info.token, codeInput);
      setCopyFeedback(result);
    });

    codeRow.append(codeInput, copyCodeBtn);
    codeGroup.append(codeLabel, codeRow);

    kioskOut.replaceChildren(urlGroup, codeGroup);
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

  kiosk.addEventListener("click", async () => {
    if (kioskCopyStatus) kioskCopyStatus.textContent = "";
    try {
      const body = await api<{ screenUrl: string; token: string }>("/api/admin/display-sessions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ label: "Kiosk" }),
      });
      lastDisplaySession = {
        fullUrl: `${location.origin}${body.screenUrl}`,
        token: body.token,
      };
      renderDisplaySession(lastDisplaySession);
    } catch (e) {
      console.error("[adminMap] create display session failed:", e);
      lastDisplaySession = null;
      kioskOut.textContent = t("api.genericError");
    }
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
    if (lastDisplaySession) {
      renderDisplaySession(lastDisplaySession);
    }
  });

  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    try {
      const me = await api<{
        pilot?: { role?: string } | null;
        clubDefaultLocale?: string;
        clubSlug?: string;
      }>("/api/club/me");
      mountClubLocalePicker(me.clubDefaultLocale ?? "es");
      if (me.clubSlug) currentClubSlug = me.clubSlug;
      if (clubSlugInput) clubSlugInput.value = me.clubSlug ?? "";
      paintSlugHelp();
      deleteClubCtrl.configure({
        isAdmin: me.pilot?.role === "admin",
        clubSlug: me.clubSlug ?? null,
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
