/** Same rules as `auth::safe_next` in the Rust API (open redirect hardening). */
export function safeNextPath(input: string): string {
  const value = input.trim();
  if (!value.startsWith("/") || value.startsWith("//")) return "/";
  if (value.includes("\\") || value.includes("://")) return "/";
  if (value === "/login" || value.startsWith("/login?")) return "/";
  if (value === "/club-login.html" || value.startsWith("/club-login.html?")) return "/";
  return value;
}

export function clubLoginRedirectUrl(next?: string): string {
  const dest = safeNextPath(
    next ?? `${window.location.pathname}${window.location.search}`,
  );
  return `/club-login.html?${new URLSearchParams({ next: dest })}`;
}

/** Club admin pages: unauthenticated users land on club login with return URL. */
export function redirectToClubLogin(next?: string): void {
  window.location.replace(clubLoginRedirectUrl(next));
}