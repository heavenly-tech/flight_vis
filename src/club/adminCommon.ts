import { t } from "../i18n/index";
import { redirectToClubLogin } from "./safeNext";

/** Set when `requireAdmin` sends the browser to club login. Admin pages should `await ensureClubAdminOrRedirect()` from `./adminGate` first (PR #20 matrix/import/audit too). */
export const CLUB_ADMIN_REDIRECT = "club-admin-redirect";

export interface PendingCounts {
  pilots: number;
  accessRequests: number;
}

export interface ClubMeSummary {
  clubName?: string | null;
  clubSlug?: string | null;
}

let adminNavLocaleBound = false;
let cachedRequestsCount = 0;
let cachedPilotsCount = 0;
let cachedClubMe: ClubMeSummary | null = null;
let clubMeFetchPromise: Promise<ClubMeSummary | null> | null = null;

const NAV_KEYS = [
  ["club-admin-types.html", "nav.types"],
  ["club-admin-tails.html", "nav.tails"],
  ["club-admin-pilots.html", "nav.pilots"],
  ["club-admin-requests.html", "nav.requests"],
  ["club-admin-checkouts.html", "nav.checkouts"],
  ["club-admin-map.html", "nav.adminMap"],
  ["club-admin-kiosk-layout.html", "nav.kioskQr"],
  ["club-admin-checkin-qr.html", "nav.adminCheckinQr"],
] as const;

export function normalizeAdminPathname(path: string): string {
  const pathOnly = path.split(/[?#]/, 1)[0] ?? "";
  const trimmed = pathOnly.replace(/\/+$/, "");
  if (!trimmed) return "/";
  return trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
}

export function isAdminNavLinkActive(linkHref: string, pathname: string): boolean {
  return normalizeAdminPathname(linkHref) === normalizeAdminPathname(pathname);
}

export function resolveActiveClubLabel(me: ClubMeSummary | null | undefined): string | null {
  const name = typeof me?.clubName === "string" ? me.clubName.trim() : "";
  if (name) return name;
  const slug = typeof me?.clubSlug === "string" ? me.clubSlug.trim() : "";
  if (slug) return slug;
  return null;
}

/** Pending access requests count shown in both the admin nav badge and Solicitudes heading. */
export function pendingAccessRequestsCount(counts: PendingCounts): number {
  const n = Number(counts.accessRequests);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
}

/** Pending pilot / membership join requests count shown in the Pilotos nav badge. */
export function pendingPilotsCount(counts: PendingCounts): number {
  const n = Number(counts.pilots);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
}

export function setNavRequestsBadge(count: number): void {
  const safe = Number.isFinite(count) && count > 0 ? Math.floor(count) : 0;
  cachedRequestsCount = safe;
  const badge = document.querySelector<HTMLSpanElement>("#admin-nav-requests-badge");
  if (!badge) return;
  badge.hidden = safe === 0;
  badge.textContent = String(safe);
}

export function setNavPilotsBadge(count: number): void {
  const safe = Number.isFinite(count) && count > 0 ? Math.floor(count) : 0;
  cachedPilotsCount = safe;
  const badge = document.querySelector<HTMLSpanElement>("#admin-nav-pilots-badge");
  if (!badge) return;
  badge.hidden = safe === 0;
  badge.textContent = String(safe);
}

export async function fetchPendingAccessRequestsCount(): Promise<number> {
  const counts = await api<PendingCounts>("/api/admin/pending-counts");
  return pendingAccessRequestsCount(counts);
}

export async function refreshNavRequestsBadge(): Promise<number | null> {
  try {
    const counts = await api<PendingCounts>("/api/admin/pending-counts");
    const count = pendingAccessRequestsCount(counts);
    setNavRequestsBadge(count);
    setNavPilotsBadge(pendingPilotsCount(counts));
    return count;
  } catch {
    return null;
  }
}

export function resetAdminNavStateForTests(): void {
  cachedRequestsCount = 0;
  cachedPilotsCount = 0;
  cachedClubMe = null;
  clubMeFetchPromise = null;
}

function paintActiveClubEl(): void {
  const clubEl = document.querySelector<HTMLSpanElement>("#admin-header-club");
  if (!clubEl) return;
  const label = resolveActiveClubLabel(cachedClubMe);
  if (label) {
    clubEl.textContent = t("nav.activeClub", { name: label });
    clubEl.hidden = false;
  } else {
    clubEl.textContent = "";
    clubEl.hidden = true;
  }
}

export function loadActiveClubMe(): Promise<ClubMeSummary | null> {
  if (clubMeFetchPromise) return clubMeFetchPromise;
  clubMeFetchPromise = (async () => {
    try {
      const me = await api<ClubMeSummary>("/api/club/me");
      cachedClubMe = {
        clubName: typeof me.clubName === "string" ? me.clubName : null,
        clubSlug: typeof me.clubSlug === "string" ? me.clubSlug : null,
      };
      paintActiveClubEl();
      return cachedClubMe;
    } catch {
      return null;
    }
  })();
  return clubMeFetchPromise;
}

export async function performAccountLogout(
  navigate: (url: string) => void = (url) => location.replace(url),
): Promise<void> {
  try {
    await fetch("/api/account/logout", {
      method: "POST",
      credentials: "same-origin",
    });
  } catch {
    /* still redirect to login */
  }
  navigate("/login");
}

function mountAdminNavInner(): void {
  const slot = document.querySelector("#admin-nav");
  if (!slot) return;

  const headerBar = document.createElement("div");
  headerBar.className = "admin-header-bar";

  const clubSpan = document.createElement("span");
  clubSpan.id = "admin-header-club";
  clubSpan.className = "admin-header-club";
  const clubLabel = resolveActiveClubLabel(cachedClubMe);
  if (clubLabel) {
    clubSpan.textContent = t("nav.activeClub", { name: clubLabel });
    clubSpan.hidden = false;
  } else {
    clubSpan.hidden = true;
  }

  const actions = document.createElement("div");
  actions.className = "admin-header-actions";

  const myClubsLink = document.createElement("a");
  myClubsLink.className = "admin-header-link";
  myClubsLink.href = "/account-clubs.html";
  myClubsLink.textContent = t("nav.myClubs");

  const myAccountLink = document.createElement("a");
  myAccountLink.className = "admin-header-link";
  myAccountLink.href = "/account.html";
  myAccountLink.textContent = t("nav.myAccount");

  const logoutBtn = document.createElement("button");
  logoutBtn.type = "button";
  logoutBtn.id = "admin-header-logout";
  logoutBtn.className = "admin-header-logout";
  logoutBtn.textContent = t("pilot.logOut");
  logoutBtn.addEventListener("click", () => {
    logoutBtn.disabled = true;
    void performAccountLogout();
  });

  actions.append(myClubsLink, myAccountLink, logoutBtn);
  headerBar.append(clubSpan, actions);

  const nav = document.createElement("nav");
  nav.className = "admin-nav";
  const currentPath = typeof location !== "undefined" ? location.pathname : "/";
  for (const [href, key] of NAV_KEYS) {
    const a = document.createElement("a");
    const target = `/${href}`;
    a.href = target;
    a.textContent = t(key);
    if (isAdminNavLinkActive(target, currentPath)) {
      a.setAttribute("aria-current", "page");
    }
    if (href === "club-admin-pilots.html") {
      const badge = document.createElement("span");
      badge.id = "admin-nav-pilots-badge";
      badge.className = "badge";
      badge.hidden = cachedPilotsCount === 0;
      badge.textContent = String(cachedPilotsCount);
      a.append(" ", badge);
    }
    if (href === "club-admin-requests.html") {
      const badge = document.createElement("span");
      badge.id = "admin-nav-requests-badge";
      badge.className = "badge";
      badge.hidden = cachedRequestsCount === 0;
      badge.textContent = String(cachedRequestsCount);
      a.append(" ", badge);
    }
    nav.appendChild(a);
  }
  slot.replaceChildren(headerBar, nav);
}

export function mountAdminNav(): void {
  if (!adminNavLocaleBound) {
    adminNavLocaleBound = true;
    window.addEventListener("fv-locale-change", () => mountAdminNavInner());
    window.addEventListener("fv-i18n-ready", () => mountAdminNavInner());
  }
  mountAdminNavInner();
  void loadActiveClubMe();
  void refreshNavRequestsBadge();
}

export function openDialog(dialog: HTMLDialogElement): void {
  if (typeof dialog.showModal === "function") {
    if (!dialog.open) dialog.showModal();
  } else {
    dialog.setAttribute("open", "");
  }
}

export function closeDialog(dialog: HTMLDialogElement): void {
  if (typeof dialog.close === "function") {
    if (dialog.open) dialog.close();
  } else {
    dialog.removeAttribute("open");
  }
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.json()) as T;
}

export async function extractApiError(res: Response): Promise<string | null> {
  try {
    const body = (await res.json()) as { error?: unknown };
    return typeof body.error === "string" && body.error.trim() ? body.error.trim() : null;
  } catch {
    return null;
  }
}

export async function requireAdmin(): Promise<{ name: string }> {
  let me: { pilot: { name: string; role: string } | null };
  try {
    me = await api<{ pilot: { name: string; role: string } | null }>("/api/club/me");
  } catch {
    redirectToClubLogin();
    throw new Error(CLUB_ADMIN_REDIRECT);
  }
  if (
    !me.pilot ||
    (me.pilot.role !== "admin" && me.pilot.role !== "staff")
  ) {
    redirectToClubLogin();
    throw new Error(CLUB_ADMIN_REDIRECT);
  }
  return { name: me.pilot.name };
}

export function isClubAdminRedirect(error: unknown): boolean {
  return error instanceof Error && error.message === CLUB_ADMIN_REDIRECT;
}
