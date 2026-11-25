import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { bundledLocaleIds, localeLabel, t } from "../i18n/index";

const list = document.querySelector<HTMLUListElement>("#live")!;
const ticks = document.querySelector<HTMLDivElement>("#ticks")!;
const save = document.querySelector<HTMLButtonElement>("#save")!;
const kiosk = document.querySelector<HTMLButtonElement>("#btn-kiosk")!;
const kioskOut = document.querySelector<HTMLParagraphElement>("#kiosk-url")!;
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

let currentClubSlug = "cas-tobalaba";

function paintSlugHelp(): void {
  const slug = currentClubSlug || "cas-tobalaba";
  if (clubLocaleHelp) {
    clubLocaleHelp.textContent = t("admin.clubDefaultLanguageHelp", { slug });
  }
  if (clubSlugHelp) {
    clubSlugHelp.textContent = t("admin.clubSlugHelp", { slug });
  }
}

async function refresh(): Promise<void> {
  const aircraft = await api<{ aircraft: { tail: string }[] }>("/api/admin/aircraft");
  const selected = await api<{ tails: string[] }>("/api/admin/map/tails");
  const set = new Set(selected.tails);
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
  const live = await api<{ aircraft: { tail: string; pilotName?: string }[] }>("/api/admin/map");
  list.replaceChildren(
    ...live.aircraft.map((row) => {
      const li = document.createElement("li");
      li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
      return li;
    }),
  );
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

kiosk?.addEventListener("click", async () => {
  try {
    const body = await api<{ screenUrl: string; token: string }>("/api/admin/display-sessions", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ label: "Kiosk" }),
    });
    const url = `${location.origin}${body.screenUrl}`;
    const link = document.createElement("a");
    link.href = url;
    link.target = "_blank";
    link.rel = "noopener";
    link.textContent = url;
    const code = document.createElement("code");
    code.textContent = body.token;
    kioskOut.replaceChildren(
      document.createTextNode(`${t("admin.kioskOpenUrl")} `),
      link,
      document.createElement("br"),
      document.createTextNode(`${t("admin.kioskActivationToken")} `),
      code,
    );
  } catch (e) {
    console.error("[adminMap] create display session failed:", e);
    kioskOut.textContent = t("api.genericError");
  }
});

save.addEventListener("click", async () => {
  try {
    const tails = [...ticks.querySelectorAll<HTMLInputElement>("input:checked")].map((i) => i.value);
    await api("/api/admin/map/tails", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ tails }),
    });
    await refresh();
  } catch (e) {
    console.error("[adminMap] save tails failed:", e);
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
    // Update inputs with normalized values
    if (data.icao) clubBaseIcaoInput.value = data.icao;
    if (data.lat !== null) clubBaseLatInput.value = String(data.lat);
    if (data.lon !== null) clubBaseLonInput.value = String(data.lon);
  } else {
    const payload = (await res.json().catch(() => ({}))) as { error?: string };
    if (clubBaseMsg) {
      clubBaseMsg.textContent = payload.error ? t("admin.invalidBase") : t("api.dbError");
    }
  }
});

void (async () => {
  if (!(await ensureClubAdminOrRedirect())) return;
  await startClubPage({ switcher: "#lang-slot" });
  mountAdminNav();
  window.addEventListener("fv-locale-change", paintSlugHelp);
  try {
    const me = await api<{ clubDefaultLocale?: string; clubSlug?: string }>("/api/club/me");
    mountClubLocalePicker(me.clubDefaultLocale ?? "es");
    if (me.clubSlug) currentClubSlug = me.clubSlug;
    if (clubSlugInput) clubSlugInput.value = me.clubSlug ?? "";
    paintSlugHelp();
    
    // Load base aerodrome
    const baseRes = await api<{ icao: string | null; lat: number | null; lon: number | null }>("/api/admin/club/base");
    if (clubBaseIcaoInput) clubBaseIcaoInput.value = baseRes.icao ?? "";
    if (clubBaseLatInput) clubBaseLatInput.value = baseRes.lat !== null ? String(baseRes.lat) : "";
    if (clubBaseLonInput) clubBaseLonInput.value = baseRes.lon !== null ? String(baseRes.lon) : "";
    
    await requireAdmin();
    await refresh();
    setInterval(() => void refresh(), 5000);
  } catch (e) {
    if (!isClubAdminRedirect(e)) list.innerHTML = `<li>${t("admin.adminRequired")}</li>`;
  }
})();