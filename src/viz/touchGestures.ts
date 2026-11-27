const GESTURE_DRAG_THRESHOLD_PX = 5;

/**
 * Attaches capture-phase pointer/touch/wheel listeners to detect user camera
 * gestures reliably across mobile touchscreens, desktop mice, and Playwright.
 * Breaks follow mode once per follow session without reacting to programmatic moves.
 */
export function attachGestureFollowBreaker(
  container: HTMLElement,
  isFollowing: () => boolean,
  onGestureBreak: () => void,
  onInteractionChange?: (active: boolean) => void,
): () => void {
  const activePointers = new Map<number, { x: number; y: number }>();
  let touchStart: { x: number; y: number } | null = null;

  const notifyActive = () => {
    onInteractionChange?.(activePointers.size > 0 || touchStart !== null);
  };

  const onPointerDown = (ev: PointerEvent) => {
    activePointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    notifyActive();
    if (activePointers.size >= 2 && isFollowing()) {
      onGestureBreak();
    }
  };

  const onPointerMove = (ev: PointerEvent) => {
    const start = activePointers.get(ev.pointerId);
    if (!start) return;
    if (
      isFollowing() &&
      Math.hypot(ev.clientX - start.x, ev.clientY - start.y) >= GESTURE_DRAG_THRESHOLD_PX
    ) {
      onGestureBreak();
    }
  };

  const onPointerEnd = (ev: PointerEvent) => {
    if (activePointers.delete(ev.pointerId)) {
      notifyActive();
    }
  };

  const onTouchStart = (ev: TouchEvent) => {
    if (ev.touches.length >= 2) {
      touchStart = null;
      notifyActive();
      if (isFollowing()) onGestureBreak();
      return;
    }
    const touch = ev.touches[0];
    if (touch) {
      touchStart = { x: touch.clientX, y: touch.clientY };
      notifyActive();
    }
  };

  const onTouchMove = (ev: TouchEvent) => {
    if (!isFollowing()) return;
    if (ev.touches.length >= 2) {
      onGestureBreak();
      return;
    }
    const touch = ev.touches[0];
    if (
      touch &&
      touchStart &&
      Math.hypot(touch.clientX - touchStart.x, touch.clientY - touchStart.y) >=
        GESTURE_DRAG_THRESHOLD_PX
    ) {
      onGestureBreak();
    }
  };

  const onTouchEnd = (ev: TouchEvent) => {
    if (ev.touches.length === 0) {
      touchStart = null;
      notifyActive();
    }
  };

  const onWheel = () => {
    if (isFollowing()) onGestureBreak();
  };

  container.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("pointermove", onPointerMove, true);
  window.addEventListener("pointerup", onPointerEnd, true);
  window.addEventListener("pointercancel", onPointerEnd, true);
  container.addEventListener("touchstart", onTouchStart, { capture: true, passive: true });
  container.addEventListener("touchmove", onTouchMove, { capture: true, passive: true });
  window.addEventListener("touchend", onTouchEnd, { capture: true, passive: true });
  window.addEventListener("touchcancel", onTouchEnd, { capture: true, passive: true });
  container.addEventListener("wheel", onWheel, { capture: true, passive: true });

  return () => {
    container.removeEventListener("pointerdown", onPointerDown, true);
    window.removeEventListener("pointermove", onPointerMove, true);
    window.removeEventListener("pointerup", onPointerEnd, true);
    window.removeEventListener("pointercancel", onPointerEnd, true);
    container.removeEventListener("touchstart", onTouchStart, true);
    container.removeEventListener("touchmove", onTouchMove, true);
    window.removeEventListener("touchend", onTouchEnd, true);
    window.removeEventListener("touchcancel", onTouchEnd, true);
    container.removeEventListener("wheel", onWheel, true);
  };
}
