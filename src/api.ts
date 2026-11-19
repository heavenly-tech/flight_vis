import type { LiveSessionSummary } from "./live/types";
import type { FlightTag } from "./track/types";
import { safeNextPath } from "./club/safeNext";

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

export type FlightListItem = {
  slug: string;
  name: string;
  fileName: string;
  updatedAt: string | null;
  createdAt: string | null;
  size: number;
  local?: boolean;
};

function throwAuthOr(res: Response, body: { error?: string }, fallback: string): never {
  if (res.status === 401) {
    throw new Error(
      body.error === "locked" ? "Upload is locked until FLIGHT_VIS_PASSWORD is set." : "Log in to manage saved flights.",
    );
  }
  throw new Error(body.error || fallback);
}

export function shareUrl(slug: string): string {
  return `${window.location.origin}/${slug}`;
}

/** Legacy disk library / flight save session (`POST /login`, `FLIGHT_VIS_PASSWORD`). */
export function loginUrl(next = window.location.pathname || "/"): string {
  const dest = safeNextPath(next);
  return `/login/legacy?next=${encodeURIComponent(dest)}`;
}

/** Public chooser (club vs pilot paths). */
export function entrarUrl(next = window.location.pathname || "/"): string {
  const dest = safeNextPath(next);
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

export async function listFlights(): Promise<FlightListItem[]> {
  const res = await fetch("/api/flights", { credentials: "same-origin" });
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throwAuthOr(res, body, "Could not list flights");
  }
  return res.json() as Promise<FlightListItem[]>;
}

export async function uploadFlights(
  files: { fileName: string; track: string; name?: string; slug?: string }[],
  conflict: "suffix" | "error" | "overwrite" = "suffix",
): Promise<{ items: FlightListItem[] }> {
  const res = await fetch("/api/flights", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ files, conflict }),
  });
  const body = (await res.json().catch(() => ({}))) as { error?: string; items?: FlightListItem[] };
  if (!res.ok) throwAuthOr(res, body, body.error || "Could not upload flights");
  return body as { items: FlightListItem[] };
}

export class SlugConflictError extends Error {
  slug: string;
  constructor(slug: string) {
    super("exists");
    this.name = "SlugConflictError";
    this.slug = slug;
  }
}

export async function renameFlight(
  slug: string,
  input: { slug?: string; name?: string; overwrite?: boolean },
): Promise<FlightListItem> {
  const res = await fetch(`/api/flights/${encodeURIComponent(slug)}`, {
    method: "PATCH",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  const body = (await res.json().catch(() => ({}))) as FlightListItem & { error?: string; slug?: string };
  if (res.status === 409) throw new SlugConflictError(body.slug || input.slug || slug);
  if (!res.ok) throwAuthOr(res, body, body.error || "Could not rename flight");
  return body;
}

export async function deleteFlight(slug: string): Promise<void> {
  const res = await fetch(`/api/flights/${encodeURIComponent(slug)}`, {
    method: "DELETE",
    credentials: "same-origin",
  });
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throwAuthOr(res, body, body.error || "Could not delete flight");
  }
}

export async function listLiveSessions(): Promise<LiveSessionSummary[]> {
  const res = await fetch("/api/live", { credentials: "same-origin" });
  if (res.status === 401 || res.status === 403) {
    throw new Error("Log in to list live sessions.");
  }
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error || "Could not list live sessions");
  }
  const body = (await res.json()) as { sessions?: LiveSessionSummary[] };
  return body.sessions ?? [];
}

export async function stopLiveSession(id: string): Promise<void> {
  const res = await fetch(`/api/live/${encodeURIComponent(id)}/stop`, {
    method: "POST",
    credentials: "same-origin",
    headers: { Accept: "application/json" },
  });
  if (res.status === 401 || res.status === 403) {
    throw new Error("Log in to stop live sessions.");
  }
  if (res.status === 404) {
    throw new Error("Live session not found");
  }
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error || "Could not stop live session");
  }
}

export async function ackLiveSession(id: string): Promise<void> {
  const res = await fetch(`/api/live/${encodeURIComponent(id)}/ack`, {
    method: "POST",
    credentials: "same-origin",
    headers: { Accept: "application/json" },
  });
  if (res.status === 401 || res.status === 403) {
    throw new Error("Log in to acknowledge live sessions.");
  }
  if (res.status === 404) {
    throw new Error("Live session not found");
  }
  if (res.status === 409) {
    throw new Error("Session still active");
  }
  if (!res.ok) {
    const body = (await res.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error || "Could not acknowledge live session");
  }
}
