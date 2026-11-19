/** Same rules as `auth::safe_next` in the Rust API (open redirect hardening). */
function percentDecodePath(input: string): string {
  let out = "";
  for (let i = 0; i < input.length; i += 1) {
    const c = input[i];
    if (c === "%" && i + 2 < input.length) {
      const hex = input.slice(i + 1, i + 3);
      const code = Number.parseInt(hex, 16);
      if (!Number.isNaN(code)) {
        out += String.fromCharCode(code);
        i += 2;
        continue;
      }
    }
    out += c;
  }
  return out;
}

function hasUnsafePathChars(s: string): boolean {
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return s.includes("\\");
}

export function safeNextPath(input: string): string {
  const raw = input.trim();
  const value = percentDecodePath(raw);
  if (hasUnsafePathChars(raw) || hasUnsafePathChars(value)) return "/";
  if (!value.startsWith("/") || value.startsWith("//")) return "/";
  if (value.includes("://")) return "/";
  if (value === "/login" || value.startsWith("/login?")) return "/";
  if (value === "/login.html" || value.startsWith("/login.html?")) return "/";
  if (value === "/login/legacy" || value.startsWith("/login/legacy?")) return "/";
  if (value === "/login-legacy.html" || value.startsWith("/login-legacy.html?")) return "/";
  if (value === "/club-login.html" || value.startsWith("/club-login.html?")) return "/";
  return value;
}

export function clubStaffLoginRedirectUrl(next?: string, club?: string): string {
  const dest = safeNextPath(
    next ?? `${window.location.pathname}${window.location.search}`,
  );
  const qs = new URLSearchParams({ mode: "club", next: dest });
  if (club?.trim()) qs.set("club", club.trim().toLowerCase());
  return `/club-login.html?${qs}`;
}

export function pilotLoginRedirectUrl(next?: string): string {
  const dest = safeNextPath(
    next ?? `${window.location.pathname}${window.location.search}`,
  );
  return `/club-login.html?${new URLSearchParams({ mode: "pilot", next: dest })}`;
}

/** @deprecated use {@link clubStaffLoginRedirectUrl} */
export function clubLoginRedirectUrl(next?: string): string {
  return clubStaffLoginRedirectUrl(next);
}

/** Club admin pages: unauthenticated users land on club login with return URL. */
export function redirectToClubLogin(next?: string): void {
  window.location.replace(clubLoginRedirectUrl(next));
}