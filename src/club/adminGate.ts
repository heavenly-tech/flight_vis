/**
 * Club-admin auth gate — await this before any admin UI logic.
 * PR #20 pages (matrix, import, audit) should call it the same way.
 */
import { redirectToClubLogin } from "./safeNext";

/** @returns false when the browser was sent to club login (stop rendering admin UI). */
export async function ensureClubAdminOrRedirect(): Promise<boolean> {
  let me: { pilot?: { role?: string } | null };
  try {
    const res = await fetch("/api/club/me", {
      credentials: "same-origin",
      signal: AbortSignal.timeout(15_000),
    });
    if (!res.ok) {
      redirectToClubLogin();
      return false;
    }
    me = (await res.json()) as { pilot?: { role?: string } | null };
  } catch {
    redirectToClubLogin();
    return false;
  }
  if (!me.pilot || me.pilot.role !== "admin") {
    redirectToClubLogin();
    return false;
  }
  return true;
}