import {
  bundledLocaleIds,
  CANONICAL_LOCALE,
  flattenCatalog,
  localeBcp47,
  localeLabel,
  type LocaleId,
} from "./catalog";

const COOKIE = "fv_locale";
const STORAGE = "fv_locale";

export type I18nState = {
  locale: string;
  messages: Record<string, string>;
  userLocale: string | null;
  clubDefaultLocale: string | null;
};

let state: I18nState = {
  locale: CANONICAL_LOCALE,
  messages: flattenCatalog(CANONICAL_LOCALE),
  userLocale: null,
  clubDefaultLocale: null,
};

function readCookie(name: string): string | null {
  const m = document.cookie.match(new RegExp(`(?:^|; )${name}=([^;]*)`));
  return m ? decodeURIComponent(m[1]) : null;
}

function writeCookie(name: string, value: string): void {
  const maxAge = 60 * 60 * 24 * 400;
  document.cookie = `${name}=${encodeURIComponent(value)}; Path=/; Max-Age=${maxAge}; SameSite=Lax`;
}

export function parseAcceptLanguage(header: string | null): string | null {
  if (!header) return null;
  const first = header.split(",")[0]?.trim().toLowerCase();
  if (!first) return null;
  const id = first.split("-")[0];
  return bundledLocaleIds().includes(id) ? id : null;
}

export function resolveLocale(opts: {
  userLocale?: string | null;
  clubDefaultLocale?: string | null;
  acceptLanguage?: string | null;
  navigatorLang?: string;
  stored?: string | null;
}): string {
  const ids = bundledLocaleIds();
  const pick = (v: string | null | undefined) =>
    v && ids.includes(v) ? v : null;
  return (
    pick(opts.stored) ??
    pick(opts.userLocale) ??
    pick(opts.clubDefaultLocale) ??
    parseAcceptLanguage(opts.acceptLanguage ?? null) ??
    (opts.navigatorLang?.toLowerCase().startsWith("es") ? "es" : null) ??
    pick(CANONICAL_LOCALE) ??
    CANONICAL_LOCALE
  );
}

export function initI18n(opts?: {
  userLocale?: string | null;
  clubDefaultLocale?: string | null;
}): I18nState {
  const stored = localStorage.getItem(STORAGE) ?? readCookie(COOKIE);
  const locale = resolveLocale({
    userLocale: opts?.userLocale,
    clubDefaultLocale: opts?.clubDefaultLocale,
    stored,
    navigatorLang: navigator.language,
  });
  state = {
    locale,
    messages: flattenCatalog(locale),
    userLocale: opts?.userLocale ?? null,
    clubDefaultLocale: opts?.clubDefaultLocale ?? null,
  };
  document.documentElement.lang = localeBcp47(locale);
  return state;
}

export function getLocale(): string {
  return state.locale;
}

export function getMessages(): Record<string, string> {
  return state.messages;
}

export function t(key: string, vars?: Record<string, string | number>): string {
  let text = state.messages[key] ?? flattenCatalog(CANONICAL_LOCALE)[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      text = text.replaceAll(`{${k}}`, String(v));
    }
  }
  return text;
}

export function formatDate(ms: number, withTime = false): string {
  const bcp = localeBcp47(state.locale);
  return new Date(ms).toLocaleString(bcp, {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    ...(withTime
      ? { hour: "2-digit", minute: "2-digit" }
      : {}),
  });
}

export function formatNumber(n: number, opts?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(localeBcp47(state.locale), opts).format(n);
}

export async function setLocale(locale: string): Promise<void> {
  if (!bundledLocaleIds().includes(locale)) return;
  state.locale = locale;
  state.messages = flattenCatalog(locale);
  document.documentElement.lang = localeBcp47(locale);
  localStorage.setItem(STORAGE, locale);
  writeCookie(COOKIE, locale);
  if (state.userLocale !== null) {
    try {
      await fetch("/api/club/me/locale", {
        method: "PATCH",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ locale }),
      });
    } catch {
      /* best-effort */
    }
    state.userLocale = locale;
  }
  applyI18nDocument();
  window.dispatchEvent(new CustomEvent("fv-locale-change"));
}

const API_ALIASES: Record<string, string> = {
  unauthorized: "api.unauthorized",
  "admin only": "api.adminOnly",
  "pending approval": "api.pendingApproval",
  "club disabled": "api.clubDisabled",
  "club missing": "api.clubMissing",
  "email exists": "api.emailExists",
  "weak password": "api.weakPassword",
  "register failed": "api.registerFailed",
  "club suspended": "api.clubSuspended",
  "invalid json": "api.invalidJson",
  "device token required": "api.deviceTokenRequired",
  "invalid device token": "api.invalidDeviceToken",
  "unknown or inactive tail": "api.unknownTail",
  "active flight exists": "api.activeFlightExists",
  "device mismatch": "api.deviceMismatch",
  "check-in required": "api.checkinRequired",
  "checkin required": "api.checkinRequired",
  "not found": "api.notFound",
  forbidden: "api.forbidden",
  "db error": "api.dbError",
  "invalid locale": "api.invalidLocale",
};

export function apiErrorMessage(raw: string, vars?: Record<string, string>): string {
  const key = API_ALIASES[raw.toLowerCase()] ?? (raw.includes("not cleared") ? "api.notCleared" : null);
  if (key) return t(key, vars);
  const slug = raw.toLowerCase().replace(/\s+/g, "_");
  const dotted = `api.${slug.replace(/_/g, (m, i) => (i ? m : ""))}`;
  if (state.messages[dotted]) return t(dotted, vars);
  return raw;
}

export function applyI18nDocument(root: ParentNode = document): void {
  const titleEl = document.querySelector<HTMLTitleElement>("title[data-i18n]");
  if (titleEl?.dataset.i18n) {
    document.title = t(titleEl.dataset.i18n);
  }
  root.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n;
    if (!key) return;
    el.textContent = t(key);
  });
  root.querySelectorAll<HTMLInputElement>("[data-i18n-placeholder]").forEach((el) => {
    const key = el.dataset.i18nPlaceholder;
    if (key) el.placeholder = t(key);
  });
  root.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    const key = el.dataset.i18nTitle;
    if (key) el.title = t(key);
  });
}

function paintLangSwitcher(slot: HTMLElement, select: HTMLSelectElement): void {
  const label = slot.querySelector("label") ?? document.createElement("label");
  label.textContent = `${t("common.language")}: `;
  select.setAttribute("aria-label", t("common.language"));
  select.replaceChildren(
    ...bundledLocaleIds().map((id) => {
      const opt = document.createElement("option");
      opt.value = id;
      opt.textContent = localeLabel(id);
      opt.selected = id === state.locale;
      return opt;
    }),
  );
  if (!slot.querySelector("label")) slot.replaceChildren(label, select);
}

export function mountLangSwitcher(slot: HTMLElement): void {
  slot.classList.add("lang-switcher");
  let select = slot.querySelector<HTMLSelectElement>("select");
  if (!select) {
    select = document.createElement("select");
    select.addEventListener("change", () => {
      void setLocale(select!.value);
    });
    slot.replaceChildren(document.createElement("label"), select);
  }
  paintLangSwitcher(slot, select);
  if (!slot.dataset.localeBound) {
    slot.dataset.localeBound = "1";
    window.addEventListener("fv-locale-change", () => {
      paintLangSwitcher(slot, select!);
    });
  }
}

export async function bootI18n(opts?: {
  userLocale?: string | null;
  clubDefaultLocale?: string | null;
  switcher?: string | HTMLElement;
}): Promise<void> {
  let userLocale = opts?.userLocale;
  let clubDefault = opts?.clubDefaultLocale;
  let dbUserLocale: string | null = null;
  try {
    const res = await fetch("/api/club/i18n/context", { credentials: "same-origin" });
    if (res.ok) {
      const ctx = (await res.json()) as {
        userLocale?: string | null;
        clubDefaultLocale?: string | null;
      };
      dbUserLocale = ctx.userLocale ?? null;
      userLocale ??= dbUserLocale;
      clubDefault ??= ctx.clubDefaultLocale ?? null;
    }
  } catch {
    /* anonymous pages */
  }
  initI18n({ userLocale: dbUserLocale, clubDefaultLocale: clubDefault });
  applyI18nDocument();
  if (dbUserLocale !== null && state.locale !== dbUserLocale) {
    await setLocale(state.locale);
  }
  const slot =
    typeof opts?.switcher === "string"
      ? document.querySelector<HTMLElement>(opts.switcher)
      : opts?.switcher;
  if (slot) mountLangSwitcher(slot);
}

export { localeLabel, localeBcp47, bundledLocaleIds, CANONICAL_LOCALE, type LocaleId };