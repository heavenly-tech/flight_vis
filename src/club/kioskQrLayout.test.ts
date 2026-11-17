import { describe, expect, it } from "vitest";
import {
  KIOSK_QR_MAX,
  KIOSK_QR_MIN,
  clampKioskQrLayout,
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
});