import { describe, expect, it } from "vitest";
import { CHROME_TAP_MAX_MS, CHROME_TAP_MOVE_PX, nextChromeTap, shouldToggleChrome } from "./globeChrome";

describe("globeChrome", () => {
  const tap = { x: 100, y: 200, t: 1000, pointerId: 7 };

  it("accepts a quick same-pointer pointerup", () => {
    expect(
      shouldToggleChrome(tap, { pointerId: 7, clientX: 105, clientY: 202, type: "pointerup" }, tap.t + 200),
    ).toBe(true);
  });

  it("rejects late pointerup (Cesium-delay scenario)", () => {
    expect(
      shouldToggleChrome(tap, { pointerId: 7, clientX: 100, clientY: 200, type: "pointerup" }, tap.t + 900),
    ).toBe(false);
  });

  it("rejects large movement", () => {
    const move = CHROME_TAP_MOVE_PX + 5;
    expect(
      shouldToggleChrome(tap, { pointerId: 7, clientX: 100 + move, clientY: 200, type: "pointerup" }, tap.t + 50),
    ).toBe(false);
  });

  it("rejects pointercancel", () => {
    expect(
      shouldToggleChrome(tap, { pointerId: 7, clientX: 100, clientY: 200, type: "pointercancel" }, tap.t + 50),
    ).toBe(false);
  });

  it("cancels tap tracking when a second pointer is already active", () => {
    expect(nextChromeTap(null, 1, { pointerId: 9, clientX: 1, clientY: 2 }, 500)).toBeNull();
  });

  it("starts tap for first pointer only", () => {
    expect(nextChromeTap(null, 0, { pointerId: 9, clientX: 1, clientY: 2 }, 500)).toEqual({
      x: 1,
      y: 2,
      t: 500,
      pointerId: 9,
    });
  });

  it("clears pending tap when pinch begins", () => {
    expect(nextChromeTap(tap, 1, { pointerId: 9, clientX: 1, clientY: 2 }, 1100)).toBeNull();
  });

  it("exposes tap timing constants for capture listeners", () => {
    expect(CHROME_TAP_MAX_MS).toBeGreaterThan(0);
    expect(CHROME_TAP_MOVE_PX).toBeGreaterThan(0);
  });
});