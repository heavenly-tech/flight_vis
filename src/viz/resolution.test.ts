import { describe, expect, it } from "vitest";
import { QUALITY_LEVELS, resolutionScaleForLevel } from "./resolution";

describe("resolutionScaleForLevel", () => {
  it("matches prod 393×852 @3× top quality (~1.5× CSS)", () => {
    expect(resolutionScaleForLevel(QUALITY_LEVELS[0].pixelCap, 3)).toBe(1.5);
    expect(Math.round(393 * resolutionScaleForLevel(1.5, 3))).toBe(590);
    expect(Math.round(852 * resolutionScaleForLevel(1.5, 3))).toBe(1278);
  });

  it("does not double-divide DPR (broken formula would yield ~0.5×)", () => {
    const wrong = Math.min(1, 1.5 / 3);
    expect(resolutionScaleForLevel(1.5, 3)).toBeGreaterThan(wrong);
    expect(Math.round(393 * wrong)).toBe(197);
  });

  it("never goes below 1 CSS pixel at the lowest governor step", () => {
    expect(resolutionScaleForLevel(QUALITY_LEVELS[2].pixelCap, 3)).toBe(1);
    expect(resolutionScaleForLevel(0.75, 2)).toBe(1);
  });

  it("caps at DPR on 1× desktop", () => {
    expect(resolutionScaleForLevel(1.5, 1)).toBe(1);
    expect(resolutionScaleForLevel(1.0, 1)).toBe(1);
  });
});