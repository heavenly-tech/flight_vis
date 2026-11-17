/** Cesium quality steps: pixelCap is the target CSS-pixel scale (≤ devicePixelRatio). */
export const QUALITY_LEVELS = [
  { pixelCap: 1.5, sse: 4 },
  { pixelCap: 1.25, sse: 6 },
  { pixelCap: 1.0, sse: 8 },
] as const;

/**
 * resolutionScale when useBrowserRecommendedResolution is true (Cesium default).
 * With that flag, scale is relative to CSS pixels and DPR is applied once by Cesium —
 * do not divide pixelCap by DPR again.
 */
export function resolutionScaleForLevel(
  pixelCap: number,
  devicePixelRatio: number = typeof globalThis !== "undefined"
    ? (globalThis as Window & typeof globalThis).devicePixelRatio || 1
    : 1,
): number {
  const dpr = Math.max(1, devicePixelRatio);
  const capped = Math.min(pixelCap, dpr);
  // Governor floor: never render below 1 CSS pixel per layout pixel.
  return Math.max(1, capped);
}