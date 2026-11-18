import { mountLangSwitcher, t } from "../i18n/index";

const NAV_KEYS = [
  ["club-admin-types.html", "nav.types"],
  ["club-admin-tails.html", "nav.tails"],
  ["club-admin-pilots.html", "nav.pilots"],
  ["club-admin-requests.html", "nav.requests"],
  ["club-admin-checkouts.html", "nav.checkouts"],
  ["club-admin-matrix.html", "nav.matrix"],
  ["club-admin-import.html", "nav.import"],
  ["club-admin-audit.html", "nav.audit"],
  ["club-admin-map.html", "nav.adminMap"],
  ["club-admin-kiosk-layout.html", "nav.kioskQr"],
  ["club-login.html", "nav.login"],
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
  const me = await api<{ pilot: { name: string; role: string } | null }>("/api/club/me");
  if (!me.pilot || me.pilot.role !== "admin") {
    throw new Error("admin");
  }
  return { name: me.pilot.name };
}