type ShareCreate = {
  id: string;
  token: string;
  url: string;
  qrSvg: string;
  createdAt: number;
};

type ShareListItem = { id: string; createdAt: number; revoked: boolean };

export async function createShare(flightId: string): Promise<ShareCreate> {
  const res = await fetch(`/api/me/flights/${encodeURIComponent(flightId)}/shares`, {
    method: "POST",
    credentials: "same-origin",
  });
  if (!res.ok) throw new Error(String(res.status));
  return (await res.json()) as ShareCreate;
}

export async function listShares(flightId: string): Promise<ShareListItem[]> {
  const res = await fetch(`/api/me/flights/${encodeURIComponent(flightId)}/shares`, {
    credentials: "same-origin",
  });
  if (!res.ok) return [];
  const body = (await res.json()) as { shares: ShareListItem[] };
  return body.shares ?? [];
}

export async function revokeShare(flightId: string, shareId: string): Promise<void> {
  await fetch(
    `/api/me/flights/${encodeURIComponent(flightId)}/shares/${encodeURIComponent(shareId)}`,
    { method: "DELETE", credentials: "same-origin" },
  );
}