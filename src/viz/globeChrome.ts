export type ChromeTap = {
  x: number;
  y: number;
  t: number;
  pointerId: number;
};

export const CHROME_TAP_MOVE_PX = 12;
export const CHROME_TAP_MAX_MS = 450;

export function shouldToggleChrome(
  tap: ChromeTap | null,
  event: { pointerId: number; clientX: number; clientY: number; type: string },
  nowMs: number,
): boolean {
  if (!tap || event.pointerId !== tap.pointerId || event.type !== "pointerup") return false;
  const dx = event.clientX - tap.x;
  const dy = event.clientY - tap.y;
  const dt = nowMs - tap.t;
  return dt <= CHROME_TAP_MAX_MS && dx * dx + dy * dy <= CHROME_TAP_MOVE_PX * CHROME_TAP_MOVE_PX;
}

/** Start a tap only for the first globe pointer; cancel if a second pointer is already down (pinch). */
export function nextChromeTap(
  priorTap: ChromeTap | null,
  activeGlobePointers: number,
  event: { pointerId: number; clientX: number; clientY: number },
  nowMs: number,
): ChromeTap | null {
  if (activeGlobePointers > 0 || priorTap !== null) return null;
  return { x: event.clientX, y: event.clientY, t: nowMs, pointerId: event.pointerId };
}