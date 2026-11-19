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
const clubLocaleSave = document.querySelector<HTMLButtonElement>("#club-locale-save");
const clubLocaleMsg = document.querySelector<HTMLParagraphElement>("#club-locale-msg");
const clubSlugInput = document.querySelector<HTMLInputElement>("#club-slug-input");
const clubSlugSave = document.querySelector<HTMLButtonElement>("#club-slug-save");
const clubSlugMsg = document.querySelector<HTMLParagraphElement>("#club-slug-msg");

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
  const body = await api<{ screenUrl: string; token: string }>("/api/admin/display-sessions", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label: "Kiosk" }),
  });
  const url = `${location.origin}${body.screenUrl}`;
  kioskOut.innerHTML = `${t("admin.kioskOpenUrl")} <a href="${url}" target="_blank" rel="noopener">${url}</a><br />
    ${t("admin.kioskActivationToken")} <code>${body.token}</code>`;
});

save.addEventListener("click", async () => {
  const tails = [...ticks.querySelectorAll<HTMLInputElement>("input:checked")].map((i) => i.value);
  await api("/api/admin/map/tails", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ tails }),
  });
  await refresh();
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
  if (res.ok && payload.slug) clubSlugInput.value = payload.slug;
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

void (async () => {
  if (!(await ensureClubAdminOrRedirect())) return;
  void startClubPage({ switcher: "#lang-slot" });
  mountAdminNav();
  try {
    const me = await api<{ clubDefaultLocale?: string; clubSlug?: string }>("/api/club/me");
    mountClubLocalePicker(me.clubDefaultLocale ?? "es");
    if (clubSlugInput) clubSlugInput.value = me.clubSlug ?? "";
    await requireAdmin();
    await refresh();
    setInterval(() => void refresh(), 5000);
  } catch (e) {
    if (!isClubAdminRedirect(e)) list.innerHTML = `<li>${t("admin.adminRequired")}</li>`;
  }
})();