/** Live status text after SSE `open` while the geoid may still be loading. */
export function liveStatusAfterStreamOpen(geoidReady: boolean, geoidPendingMsg: string): string | null {
  return geoidReady ? null : geoidPendingMsg;
}