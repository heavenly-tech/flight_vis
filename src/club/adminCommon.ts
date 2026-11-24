import { t } from "../i18n/index";
import { redirectToClubLogin } from "./safeNext";

/** Set when `requireAdmin` sends the browser to club login. Admin pages should `await ensureClubAdminOrRedirect()` from `./adminGate` first (PR #20 matrix/import/audit too). */
export const CLUB_ADMIN_REDIRECT = "club-admin-redirect";

export interface PendingCounts {
  pilots: number;
  accessRequests: number;
}

let adminNavLocaleBound = false;
let cachedRequestsCount = 0;

const NAV_KEYS = [
  ["club-admin-types.html", "nav.types"],
  ["club-admin-tails.html", "nav.tails"],
  ["club-admin-pilots.html", "nav.pilots"],
  ["club-admin-requests.html", "nav.requests"],
  ["club-admin-checkouts.html", "nav.checkouts"],
  ["club-admin-map.html", "nav.adminMap"],
  ["club-admin-kiosk-layout.html", "nav.kioskQr"],
  ["club-admin-checkin-qr.html", "nav.adminCheckinQr"],
  ["login?mode=club", "nav.login"],
] as const;

/** Pending access requests count shown in both the admin nav badge and Solicitudes heading. */
export function pendingAccessRequestsCount(counts: PendingCounts): number {
  const n = Number(counts.accessRequests);
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

export async function fetchPendingAccessRequestsCount(): Promise<number> {
  const counts = await api<PendingCounts>("/api/admin/pending-counts");
  return pendingAccessRequestsCount(counts);
}

export async function refreshNavRequestsBadge(): Promise<number | null> {
  try {
    const count = await fetchPendingAccessRequestsCount();
    setNavRequestsBadge(count);
    return count;
  } catch {
    return null;
  }
}

export function resetAdminNavStateForTests(): void {
  cachedRequestsCount = 0;
}

function mountAdminNavInner(): void {
  const slot = document.querySelector("#admin-nav");
  if (!slot) return;
  const nav = document.createElement("nav");
  nav.className = "admin-nav";
  for (const [href, key] of NAV_KEYS) {
    const a = document.createElement("a");
    a.href = `/${href}`;
    a.textContent = t(key);
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
  slot.replaceChildren(nav);
}

export function mountAdminNav(): void {
  if (!adminNavLocaleBound) {
    adminNavLocaleBound = true;
    window.addEventListener("fv-locale-change", () => mountAdminNavInner());
    window.addEventListener("fv-i18n-ready", () => mountAdminNavInner());
  }
  mountAdminNavInner();
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
