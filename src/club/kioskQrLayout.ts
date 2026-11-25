/** Shared kiosk QR placement math (matches server `kiosk_qr.rs`). */

export const KIOSK_QR_MIN = 0.18;
export const KIOSK_QR_MAX = 0.5;
export const KIOSK_QR_MARGIN = 0.02;
export const DEFAULT_PREVIEW_ASPECT = 16 / 9;
export const NARROW_VIEWPORT_MAX_WIDTH = 480;
export const NARROW_QR_MAX_WIDTH_RATIO = 0.28;
export const NARROW_QR_MIN_SIDE_PX = 96;

export type KioskQrLayout = {
  x: number;
  y: number;
  size: number;
  rev?: number;
  checkinQrVisible?: boolean;
};

export function qrFootprintFractions(
  aspect: number,
  size: number,
): { sw: number; sh: number } {
  const w = aspect >= 1 ? aspect : 1;
  const h = aspect >= 1 ? 1 : 1 / aspect;
  const short = Math.min(w, h);
  return { sw: (size * short) / w, sh: (size * short) / h };
}

export function clampKioskQrLayout(
  x: number,
  y: number,
  size: number,
  aspect: number,
): KioskQrLayout {
  const sz = Math.min(KIOSK_QR_MAX, Math.max(KIOSK_QR_MIN, size));
  const { sw, sh } = qrFootprintFractions(aspect, sz);
  const maxX = Math.max(0, 1 - sw);
  const maxY = Math.max(0, 1 - sh);
  return {
    x: Math.min(maxX, Math.max(0, x)),
    y: Math.min(maxY, Math.max(0, y)),
    size: sz,
  };
}

export function defaultKioskQrLayout(aspect: number): KioskQrLayout {
  const size = 0.22;
  const { sw, sh } = qrFootprintFractions(aspect, size);
  return {
    x: Math.max(0, 1 - sw - KIOSK_QR_MARGIN),
    y: Math.max(0, 1 - sh - KIOSK_QR_MARGIN),
    size,
  };
}

export function computeQrBoxPx(
  layout: KioskQrLayout,
  rect: { width: number; height: number },
): { left: number; top: number; side: number } {
  const width = Math.max(0, rect.width);
  const height = Math.max(0, rect.height);
  const short = Math.min(width, height);
  let side = Math.max(0, layout.size * short);
  if (width > 0 && width <= NARROW_VIEWPORT_MAX_WIDTH) {
    const narrowCap = Math.min(
      short,
      Math.max(NARROW_QR_MIN_SIDE_PX, width * NARROW_QR_MAX_WIDTH_RATIO),
    );
    side = Math.min(side, narrowCap);
  } else {
    side = Math.min(side, short);
  }
  const maxLeft = Math.max(0, width - side);
  const maxTop = Math.max(0, height - side);
  const left = Math.max(0, Math.min(layout.x * width, maxLeft));
  const top = Math.max(0, Math.min(layout.y * height, maxTop));
  return { left, top, side };
}

export function applyLayoutToEl(
  el: HTMLElement,
  layout: KioskQrLayout,
  viewportEl: HTMLElement,
): void {
  const rect = viewportEl.getBoundingClientRect();
  const { left, top, side } = computeQrBoxPx(layout, rect);
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
  el.style.width = `${side}px`;
  el.style.height = `${side}px`;
}