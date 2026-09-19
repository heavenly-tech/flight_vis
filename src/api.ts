import type { FlightTag } from "./track/types";

export type FlightRecord = {
  slug: string;
  name: string;
  fileName: string;
  trimStartMs: number | null;
  trimEndMs: number | null;
  tags: FlightTag[];
  track: string;
  updatedAt: string;
};

export type AuthStatus = {
  authenticated: boolean;
  configured: boolean;
};

export function shareUrl(slug: string): string {
  return `${window.location.origin}/${slug}`;
}

export function loginUrl(next = window.location.pathname || "/"): string {
  const dest = next.startsWith("/") && !next.startsWith("//") ? next : "/";
  return `/login?next=${encodeURIComponent(dest)}`;
}

export async function getAuth(): Promise<AuthStatus> {
  const res = await fetch("/api/auth/me", { credentials: "same-origin" });
  if (!res.ok) return { authenticated: false, configured: true };
  return res.json() as Promise<AuthStatus>;
}

export async function logout(): Promise<void> {
  await fetch("/logout", {
    method: "POST",
    credentials: "same-origin",
    headers: { Accept: "application/json" },
  });
}

export async function getFlight(slug: string): Promise<FlightRecord | null> {
  const res = await fetch(`/api/flights/${encodeURIComponent(slug)}`, { credentials: "same-origin" });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error("Could not load flight");
  return res.json() as Promise<FlightRecord>;
}

export async function saveFlight(input: {
  slug: string;
  name: string;
  fileName: string;
  trimStartMs: number | null;
  trimEndMs: number | null;
  tags: FlightTag[];
  track?: string;
}): Promise<FlightRecord> {
  const res = await fetch(`/api/flights/${encodeURIComponent(input.slug)}`, {
    method: "PUT",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (res.status === 401) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error === "locked" ? "Upload is locked until FLIGHT_VIS_PASSWORD is set." : "Log in to save this flight.");
  }
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || "Could not save flight");
  }
  return res.json() as Promise<FlightRecord>;
}
