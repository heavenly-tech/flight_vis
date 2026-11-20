import { describe, expect, it } from "vitest";
import { clampOverlayRect, MIN_OVERLAY_SIZE, MAX_OVERLAY_SIZE } from "./globeAdminQr";

describe("clampOverlayRect", () => {
  const viewport = { width: 1000, height: 800 };
  const minSize = MIN_OVERLAY_SIZE;

  it("keeps valid rect unchanged", () => {
    const rect = { x: 100, y: 100, width: 300, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result).toEqual(rect);
  });

  it("clamps too-small width to minSize", () => {
    const rect = { x: 100, y: 100, width: 100, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.width).toBe(minSize);
    expect(result.height).toBe(300);
  });

  it("clamps too-small height to minSize", () => {
    const rect = { x: 100, y: 100, width: 300, height: 100 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.width).toBe(300);
    expect(result.height).toBe(minSize);
  });

  it("clamps too-large width to viewport width", () => {
    const rect = { x: 100, y: 100, width: 1200, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.width).toBe(viewport.width);
    expect(result.height).toBe(300);
  });

  it("clamps too-large height to viewport height", () => {
    const rect = { x: 100, y: 100, width: 300, height: 1200 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.width).toBe(300);
    expect(result.height).toBe(viewport.height);
  });

  it("clamps off-screen left position to 0", () => {
    const rect = { x: -100, y: 100, width: 300, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.x).toBe(0);
    expect(result.y).toBe(100);
  });

  it("clamps off-screen top position to 0", () => {
    const rect = { x: 100, y: -100, width: 300, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.x).toBe(100);
    expect(result.y).toBe(0);
  });

  it("clamps off-screen right position to keep overlay within viewport", () => {
    const rect = { x: 900, y: 100, width: 300, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.x).toBe(viewport.width - 300);
    expect(result.y).toBe(100);
  });

  it("clamps off-screen bottom position to keep overlay within viewport", () => {
    const rect = { x: 100, y: 700, width: 300, height: 300 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.x).toBe(100);
    expect(result.y).toBe(viewport.height - 300);
  });

  it("clamps both position and size when needed", () => {
    const rect = { x: -50, y: -50, width: 1200, height: 1200 };
    const result = clampOverlayRect(rect, viewport, minSize);
    expect(result.x).toBe(0);
    expect(result.y).toBe(0);
    expect(result.width).toBe(viewport.width);
    expect(result.height).toBe(viewport.height);
  });

  it("handles very small viewport", () => {
    const smallViewport = { width: 200, height: 200 };
    const rect = { x: 0, y: 0, width: 300, height: 300 };
    const result = clampOverlayRect(rect, smallViewport, minSize);
    expect(result.width).toBe(smallViewport.width);
    expect(result.height).toBe(smallViewport.height);
  });

  it("respects custom minSize parameter", () => {
    const rect = { x: 100, y: 100, width: 100, height: 100 };
    const result = clampOverlayRect(rect, viewport, 200);
    expect(result.width).toBe(200);
    expect(result.height).toBe(200);
  });

  it("clamps to max size when rect exceeds MAX_OVERLAY_SIZE", () => {
    // Use a larger viewport so MAX_OVERLAY_SIZE is the limiting factor
    const largeViewport = { width: 2000, height: 2000 };
    const rect = { x: 100, y: 100, width: MAX_OVERLAY_SIZE + 100, height: MAX_OVERLAY_SIZE + 100 };
    const result = clampOverlayRect(rect, largeViewport, minSize);
    expect(result.width).toBe(MAX_OVERLAY_SIZE);
    expect(result.height).toBe(MAX_OVERLAY_SIZE);
  });
});
