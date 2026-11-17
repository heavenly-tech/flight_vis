import { loginUrl } from "../api";
import type { LiveFix, LiveSessionSummary } from "./types";

export async function listLiveSessions(): Promise<LiveSessionSummary[]> {
  const res = await fetch("/api/live", { credentials: "same-origin" });
  if (res.status === 401) {
    throw new Error("auth");
  }
  if (!res.ok) {
    throw new Error("Could not list live sessions");
  }
  const body = (await res.json()) as { sessions?: LiveSessionSummary[] };
  return body.sessions ?? [];
}

export async function fetchLiveFixes(sessionId: string): Promise<LiveFix[] | null> {
  const res = await fetch(`/api/live/${encodeURIComponent(sessionId)}/fixes`, {
    credentials: "same-origin",
  });
  if (res.status === 401) throw new Error("auth");
  if (res.status === 404) return null;
  if (!res.ok) throw new Error("Could not load live fixes");
  const body = (await res.json()) as { fixes?: LiveFix[] };
  return body.fixes ?? [];
}

export function liveStreamUrl(sessionId: string): string {
  return `/api/live/${encodeURIComponent(sessionId)}/stream`;
}

export { loginUrl };