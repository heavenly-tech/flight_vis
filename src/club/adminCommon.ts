const NAV = `
<nav class="admin-nav">
  <a href="/club-admin-types.html">Types</a>
  <a href="/club-admin-tails.html">Tails</a>
  <a href="/club-admin-pilots.html">Pilots</a>
  <a href="/club-admin-checkouts.html">Checkouts</a>
  <a href="/club-admin-map.html">Admin map</a>
  <a href="/club-login.html">Login</a>
</nav>`;

export function mountAdminNav(): void {
  const slot = document.querySelector("#admin-nav");
  if (slot) slot.innerHTML = NAV;
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.json()) as T;
}

export async function requireAdmin(): Promise<{ name: string }> {
  const me = await api<{ pilot: { name: string; role: string } | null }>("/api/club/me");
  if (!me.pilot || me.pilot.role !== "admin") {
    throw new Error("admin required");
  }
  return { name: me.pilot.name };
}