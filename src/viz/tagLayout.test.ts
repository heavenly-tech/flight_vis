import { describe, expect, it } from "vitest";
import {
  computeTagLabelLayouts,
  TAG_CLUSTER_SECONDARY_MAX_DISTANCE_M,
  TAG_LABEL_MAX_DISTANCE_M,
} from "./tagLayout";

describe("computeTagLabelLayouts", () => {
  it("returns an empty map for empty input", () => {
    expect(computeTagLabelLayouts([]).size).toBe(0);
  });

  it("places well-separated tags above their anchor with full display distance", () => {
    const layouts = computeTagLabelLayouts([
      { id: "a", x: 0, y: 0, z: 0 },
      { id: "b", x: 10_000, y: 0, z: 0 },
    ]);
    const a = layouts.get("a")!;
    const b = layouts.get("b")!;
    expect(a.verticalOrigin).toBe("BOTTOM");
    expect(a.offsetY).toBeLessThan(0);
    expect(a.maxDisplayDistanceM).toBe(TAG_LABEL_MAX_DISTANCE_M);
    expect(b.verticalOrigin).toBe("BOTTOM");
    expect(b.offsetY).toBe(a.offsetY);
    expect(b.maxDisplayDistanceM).toBe(TAG_LABEL_MAX_DISTANCE_M);
  });

  it("staggers clustered tags into distinct offsets and shortens secondary display distance", () => {
    const layouts = computeTagLabelLayouts([
      { id: "a", x: 0, y: 0, z: 0 },
      { id: "b", x: 500, y: 0, z: 0 },
      { id: "c", x: 1000, y: 0, z: 0 },
    ]);
    const a = layouts.get("a")!;
    const b = layouts.get("b")!;
    const c = layouts.get("c")!;

    expect(a.verticalOrigin).toBe("BOTTOM");
    expect(b.verticalOrigin).toBe("TOP");
    expect(new Set([a.offsetY, b.offsetY, c.offsetY]).size).toBe(3);
    expect(a.maxDisplayDistanceM).toBe(TAG_LABEL_MAX_DISTANCE_M);
    expect(b.maxDisplayDistanceM).toBe(TAG_CLUSTER_SECONDARY_MAX_DISTANCE_M);
    expect(c.maxDisplayDistanceM).toBe(TAG_CLUSTER_SECONDARY_MAX_DISTANCE_M);
  });
});
