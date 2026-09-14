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

export function shareUrl(slug: string): string {
  return `${window.location.origin}/${slug}`;
}

export async function getFlight(slug: string): Promise<FlightRecord | null> {
  const res = await fetch(`/api/flights/${encodeURIComponent(slug)}`);
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
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || "Could not save flight");
  }
  return res.json() as Promise<FlightRecord>;
}
