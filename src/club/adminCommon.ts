import { mountLangSwitcher, t } from "../i18n/index";
import { redirectToClubLogin } from "./safeNext";

/** Set when `requireAdmin` sends the browser to club login. Admin pages should `await ensureClubAdminOrRedirect()` from `./adminGate` first (PR #20 matrix/import/audit too). */
export const CLUB_ADMIN_REDIRECT = "club-admin-redirect";

const NAV_KEYS = [
  ["club-admin-types.html", "nav.types"],
  ["club-admin-tails.html", "nav.tails"],
  ["club-admin-pilots.html", "nav.pilots"],
  ["club-admin-requests.html", "nav.requests"],
  ["club-admin-checkouts.html", "nav.checkouts"],
  ["club-admin-map.html", "nav.adminMap"],
  ["club-admin-kiosk-layout.html", "nav.kioskQr"],
  ["/login?mode=club", "nav.login"],
] as const;

export function mountAdminNav(): void {
  const slot = document.querySelector("#admin-nav");
  if (!slot) return;
  const nav = document.createElement("nav");
  nav.className = "admin-nav";
  for (const [href, key] of NAV_KEYS) {
    const a = document.createElement("a");
    a.href = `/${href}`;
    a.textContent = t(key);
    nav.appendChild(a);
  }
  slot.replaceChildren(nav);
}

export function mountAdminLangSwitcher(): void {
  const slot = document.querySelector("#lang-slot");
  if (slot instanceof HTMLElement) mountLangSwitcher(slot, { persistUser: true });
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.json()) as T;
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