/**
 * Cheap, deterministic layout offsets and distance display limits for Cesium
 * flight event-tag labels so adjacent tags along a track avoid stacking.
 */
export type TagAnchor = {
  id: string;
  x: number;
  y: number;
  z: number;
};

export type TagLabelPlacement = {
  id: string;
  offsetX: number;
  offsetY: number;
  verticalOrigin: "BOTTOM" | "TOP";
  maxDisplayDistanceM: number;
};

export const TAG_CLUSTER_DISTANCE_M = 3_500;
export const TAG_LABEL_MAX_DISTANCE_M = 75_000;
export const TAG_CLUSTER_SECONDARY_MAX_DISTANCE_M = 35_000;

const SLOTS: ReadonlyArray<{
  offsetX: number;
  offsetY: number;
  verticalOrigin: "BOTTOM" | "TOP";
}> = [
  { offsetX: 0, offsetY: -16, verticalOrigin: "BOTTOM" },
  { offsetX: 0, offsetY: 16, verticalOrigin: "TOP" },
  { offsetX: 0, offsetY: -38, verticalOrigin: "BOTTOM" },
  { offsetX: 0, offsetY: 38, verticalOrigin: "TOP" },
];

export function computeTagLabelLayouts(
  anchors: TagAnchor[],
  opts?: {
    clusterDistanceM?: number;
    maxDisplayDistanceM?: number;
    clusterSecondaryMaxDistanceM?: number;
  },
): Map<string, TagLabelPlacement> {
  const clusterDist = opts?.clusterDistanceM ?? TAG_CLUSTER_DISTANCE_M;
  const maxDist = opts?.maxDisplayDistanceM ?? TAG_LABEL_MAX_DISTANCE_M;
  const secondaryMaxDist =
    opts?.clusterSecondaryMaxDistanceM ?? TAG_CLUSTER_SECONDARY_MAX_DISTANCE_M;

  const placed: Array<{ anchor: TagAnchor; slot: number }> = [];
  const layouts = new Map<string, TagLabelPlacement>();

  for (const anchor of anchors) {
    const usedSlots = new Set<number>();
    for (const prev of placed) {
      const dist = Math.hypot(
        anchor.x - prev.anchor.x,
        anchor.y - prev.anchor.y,
        anchor.z - prev.anchor.z,
      );
      if (dist <= clusterDist) {
        usedSlots.add(prev.slot);
      }
    }
    let slot = 0;
    while (usedSlots.has(slot) && slot < SLOTS.length - 1) {
      slot += 1;
    }
    const chosen = SLOTS[slot];
    placed.push({ anchor, slot });
    layouts.set(anchor.id, {
      id: anchor.id,
      offsetX: chosen.offsetX,
      offsetY: chosen.offsetY,
      verticalOrigin: chosen.verticalOrigin,
      maxDisplayDistanceM: usedSlots.size > 0 ? secondaryMaxDist : maxDist,
    });
  }

  return layouts;
}
