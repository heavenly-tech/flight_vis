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
    pick(parseAcceptLanguage(opts.acceptLanguage ?? null)) ??
    pick(opts.clubDefaultLocale) ??
    pick(CANONICAL_LOCALE) ??
    CANONICAL_LOCALE
  );
}

export function initI18n(opts?: {
  userLocale?: string | null;
  clubDefaultLocale?: string | null;
  acceptLanguage?: string | null;
  stored?: string | null;
}): I18nState {
  const stored =
    opts?.stored ?? localStorage.getItem(STORAGE) ?? readCookie(COOKIE);
  const browserLang =
    opts?.acceptLanguage ??
    (typeof navigator !== "undefined" ? navigator.language : null);
  const locale = resolveLocale({
    userLocale: opts?.userLocale,
    clubDefaultLocale: opts?.clubDefaultLocale,
    acceptLanguage: browserLang,
    stored,
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

/** When a signed-in club user picks a locale, persist it to the account (shared-browser case). */
export async function setLocale(
  locale: string,
  opts?: { persistUser?: boolean },
): Promise<void> {
  if (!bundledLocaleIds().includes(locale)) return;
  state.locale = locale;
  state.messages = flattenCatalog(locale);
  document.documentElement.lang = localeBcp47(locale);
  localStorage.setItem(STORAGE, locale);
  writeCookie(COOKIE, locale);
  const persist = opts?.persistUser ?? state.userLocale !== null;
  if (persist) {
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
    if (state.userLocale !== null) {
      state.userLocale = locale;
    }
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
  "club not found": "api.clubMissing",
  "club closed": "api.clubMissing",
  invalid_club: "api.clubMissing",
  "email exists": "api.emailExists",
  "weak password": "api.weakPassword",
  "register failed": "api.registerFailed",
  "signup failed": "api.registerFailed",
  "join failed": "api.joinFailed",
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
  "invalid slug": "api.invalidSlug",
  "slug mismatch": "adminMap.deleteClubWrongSlug",
  "slug taken": "api.slugTaken",
  unchanged: "api.unchanged",
  locked: "globe.uploadLocked",
  "too many attempts": "api.rateLimited",
  "rate limited": "api.rateLimited",
  email_not_verified: "api.emailNotVerified",
  club_visibility_requires_club: "api.clubVisibilityRequiresClub",
  "invalid tail": "api.invalidTail",
  leave_clubs_first: "api.leaveClubsFirst",
  "batch too large": "api.batchTooLarge",
  csrf: "api.forbidden",
  last_admin: "club.admin.lastAdmin",
  "invalid icao": "admin.invalidBase",
  "invalid lat": "admin.invalidBase",
  "invalid lon": "admin.invalidBase",
  "invalid layout": "api.genericError",
  exists: "api.slugTaken",
  "unsupported media type": "api.genericError",
  "session still active": "api.activeFlightExists",
};

export function apiErrorMessage(raw: string, vars?: Record<string, string>): string {
  const normalized = raw.trim().toLowerCase();
  const key = API_ALIASES[normalized] ?? (normalized.includes("not cleared") ? "api.notCleared" : null);
  if (key) return t(key, vars);
  const slug = normalized.replace(/\s+/g, "_");
  const dotted = `api.${slug.replace(/_/g, (m, i) => (i ? m : ""))}`;
  if (state.messages[dotted]) return t(dotted, vars);
  if (raw.trim()) console.warn("[i18n] unmapped api error:", raw);
  return t("api.genericError", vars);
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
  root.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((el) => {
    const key = el.dataset.i18nAria;
    if (key) el.setAttribute("aria-label", t(key));
  });
  root.querySelectorAll<HTMLImageElement>("[data-i18n-alt]").forEach((el) => {
    const key = el.dataset.i18nAlt;
    if (key) el.alt = t(key);
  });
  const metaI18n = document.querySelector<HTMLMetaElement>('meta[data-i18n-meta]');
  if (metaI18n?.dataset.i18nMeta) {
    metaI18n.content = t(metaI18n.dataset.i18nMeta);
  }
}

function paintLangSwitcher(slot: HTMLElement, select: HTMLSelectElement): void {
  const label = slot.querySelector("label") ?? document.createElement("label");
  if (!select.id) select.id = "fv-lang-select";
  label.htmlFor = select.id;
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

export function mountLangSwitcher(
  slot: HTMLElement,
  opts?: { persistUser?: boolean },
): void {
  slot.classList.add("lang-switcher");
  let select = slot.querySelector<HTMLSelectElement>("select");
  if (!select) {
    select = document.createElement("select");
    select.addEventListener("change", () => {
      void setLocale(select!.value, opts).then(() => {
        location.reload();
      });
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
  persistUser?: boolean;
  switcher?: string | HTMLElement;
}): Promise<void> {
  let userLocale = opts?.userLocale;
  let clubDefault = opts?.clubDefaultLocale;
  let dbUserLocale: string | null = null;
  let serverResolved: string | null = null;
  const browserLang =
    typeof navigator !== "undefined" ? navigator.language : null;
  const stored = localStorage.getItem(STORAGE) ?? readCookie(COOKIE);
  try {
    const res = await fetch("/api/club/i18n/context", {
      credentials: "same-origin",
      signal: AbortSignal.timeout(10_000),
    });
    if (res.ok) {
      const ctx = (await res.json()) as {
        userLocale?: string | null;
        clubDefaultLocale?: string | null;
        resolvedLocale?: string | null;
      };
      dbUserLocale = ctx.userLocale ?? null;
      userLocale ??= dbUserLocale;
      clubDefault ??= ctx.clubDefaultLocale ?? null;
      serverResolved = ctx.resolvedLocale ?? null;
    }
  } catch {
    /* anonymous pages */
  }
  initI18n({
    userLocale: dbUserLocale,
    clubDefaultLocale: clubDefault,
    acceptLanguage: browserLang,
    stored,
  });
  if (
    serverResolved &&
    bundledLocaleIds().includes(serverResolved) &&
    state.locale !== serverResolved
  ) {
    state.locale = serverResolved;
    state.messages = flattenCatalog(serverResolved);
    document.documentElement.lang = localeBcp47(serverResolved);
  }
  applyI18nDocument();
  // Browser-stored locale overrides DB on boot; sync back when the viewer has a club session.
  if (dbUserLocale !== null && state.locale !== dbUserLocale) {
    void setLocale(state.locale);
  }
  const slot =
    typeof opts?.switcher === "string"
      ? document.querySelector<HTMLElement>(opts.switcher)
      : opts?.switcher;
  if (slot) {
    mountLangSwitcher(slot, {
      persistUser: opts?.persistUser ?? dbUserLocale !== null,
    });
  }
  window.dispatchEvent(new CustomEvent("fv-i18n-ready"));
}

export { localeLabel, localeBcp47, bundledLocaleIds, CANONICAL_LOCALE, type LocaleId };