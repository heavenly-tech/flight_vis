import { describe, expect, it } from "vitest";
import {
  KIOSK_QR_MAX,
  KIOSK_QR_MIN,
  NARROW_QR_MAX_WIDTH_RATIO,
  NARROW_QR_MIN_SIDE_PX,
  applyLayoutToEl,
  clampKioskQrLayout,
  computeQrBoxPx,
  defaultKioskQrLayout,
  qrFootprintFractions,
} from "./kioskQrLayout";

describe("kioskQrLayout", () => {
  it("clamps size to bounds", () => {
    expect(clampKioskQrLayout(0, 0, 0.01, 16 / 9).size).toBe(KIOSK_QR_MIN);
    expect(clampKioskQrLayout(0, 0, 1, 16 / 9).size).toBe(KIOSK_QR_MAX);
  });

  it("keeps box inside viewport", () => {
    const layout = clampKioskQrLayout(0.9, 0.9, 0.4, 16 / 9);
    const { sw, sh } = qrFootprintFractions(16 / 9, layout.size);
    expect(layout.x + sw).toBeLessThanOrEqual(1.0001);
    expect(layout.y + sh).toBeLessThanOrEqual(1.0001);
  });

  it("default anchors bottom-right", () => {
    const d = defaultKioskQrLayout(16 / 9);
    const { sw, sh } = qrFootprintFractions(16 / 9, d.size);
    expect(d.x + sw).toBeGreaterThan(0.94);
    expect(d.y + sh).toBeGreaterThan(0.94);
  });

  it("clamps default 16:9 layout fully inside a 390x844 mobile viewport", () => {
    const layout = defaultKioskQrLayout(16 / 9);
    const box = computeQrBoxPx(layout, { width: 390, height: 844 });
    expect(box.left).toBeGreaterThanOrEqual(0);
    expect(box.top).toBeGreaterThanOrEqual(0);
    expect(box.left + box.side).toBeLessThanOrEqual(390.001);
    expect(box.top + box.side).toBeLessThanOrEqual(844.001);
    expect(box.side).toBeLessThanOrEqual(390 * NARROW_QR_MAX_WIDTH_RATIO + 0.001);
  });

  it("shrinks large QR to at most 28% width on narrow viewports (<=480px) with 96px minimum when it fits", () => {
    // At 390px wide, 28% is 109.2px (> 96px), so size=0.5 (195px) shrinks to 109.2px.
    const box390 = computeQrBoxPx({ x: 0.9, y: 0.9, size: 0.5 }, { width: 390, height: 844 });
    expect(box390.side).toBeCloseTo(390 * NARROW_QR_MAX_WIDTH_RATIO, 3);
    expect(box390.left + box390.side).toBeLessThanOrEqual(390.001);
    expect(box390.top + box390.side).toBeLessThanOrEqual(844.001);

    // At 320px wide, 28% is 89.6px (< 96px), so minimum 96px applies because 96 <= 320 fits.
    const box320 = computeQrBoxPx({ x: 0.9, y: 0.9, size: 0.5 }, { width: 320, height: 568 });
    expect(box320.side).toBe(NARROW_QR_MIN_SIDE_PX);
    expect(box320.left).toBe(320 - NARROW_QR_MIN_SIDE_PX);
    expect(box320.top + box320.side).toBeLessThanOrEqual(568.001);

    // On a tiny 80x80 viewport where 96px does not fit, side stays <= 80px.
    const boxTiny = computeQrBoxPx({ x: 0.9, y: 0.9, size: 0.5 }, { width: 80, height: 80 });
    expect(boxTiny.side).toBe(40);
    expect(boxTiny.left + boxTiny.side).toBeLessThanOrEqual(80.001);
  });

  it("clamps negative and oversized coordinates in applyLayoutToEl", () => {
    const viewport = document.createElement("div");
    viewport.getBoundingClientRect = () =>
      ({
        left: 0,
        top: 0,
        right: 390,
        bottom: 844,
        width: 390,
        height: 844,
        x: 0,
        y: 0,
        toJSON: () => ({}),
      }) as DOMRect;
    const qrEl = document.createElement("div");

    applyLayoutToEl(qrEl, { x: 0.95, y: 0.98, size: 0.4 }, viewport);
    const left = parseFloat(qrEl.style.left);
    const top = parseFloat(qrEl.style.top);
    const width = parseFloat(qrEl.style.width);
    const height = parseFloat(qrEl.style.height);
    expect(width).toBeCloseTo(390 * NARROW_QR_MAX_WIDTH_RATIO, 3);
    expect(height).toBeCloseTo(width, 3);
    expect(left).toBeGreaterThanOrEqual(0);
    expect(top).toBeGreaterThanOrEqual(0);
    expect(left + width).toBeLessThanOrEqual(390.001);
    expect(top + height).toBeLessThanOrEqual(844.001);

    applyLayoutToEl(qrEl, { x: -0.2, y: -0.5, size: 0.22 }, viewport);
    expect(parseFloat(qrEl.style.left)).toBe(0);
    expect(parseFloat(qrEl.style.top)).toBe(0);
  });
});
