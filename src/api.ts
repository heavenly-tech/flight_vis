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

export type FlightListItem = {
  slug: string;
  name: string;
  fileName: string;
  updatedAt: string | null;
  createdAt: string | null;
  size: number;
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
