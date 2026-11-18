const LIVE_PATH = /^\/live(?:\/([A-Za-z0-9._-]+))?\/?$/i;

export function parseLivePath(pathname: string): { listOnly: boolean; sessionId: string | null } {
  const match = pathname.match(LIVE_PATH);
  if (!match) return { listOnly: false, sessionId: null };
  return { listOnly: !match[1], sessionId: match[1] ?? null };
}