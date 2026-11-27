import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { CameraEventType, type ScreenSpaceCameraController } from "cesium";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_GLOBE_SCENE_OPTIONS,
  createQualityGovernor,
  resolutionScaleForOptions,
  resolveGlobeSceneOptions,
} from "./globeScene";
import { QUALITY_LEVELS } from "./resolution";
import {
  MOBILE_TOUCH_INERTIA,
  configureMobileTouchControls,
  formatAircraftLabelText,
  mslPointToEllipsoidAlt,
} from "./multiScene";
import {
  ensureGeoidLoaded,
  geoidUndulationM,
  installGeoidGrid,
  mslToEllipsoid,
  resetGeoidForTests,
  type GeoidMeta,
} from "../geo/geoid";
import { initI18n } from "../i18n/index";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("globeScene pure options & quality governor", () => {
  it("resolves default options matching the web viewer baseline", () => {
    expect(resolveGlobeSceneOptions()).toEqual({
      creditContainer: undefined,
      ...DEFAULT_GLOBE_SCENE_OPTIONS,
    });
  });

  it("clamps custom mobile options safely", () => {
    expect(
      resolveGlobeSceneOptions({
        msaaSamples: 1,
        maxResolutionScale: 1.25,
        maximumRequestsPerServer: 6,
        tileCacheSize: 128,
        minimumZoomDistance: 75,
        maximumZoomDistance: 500_000,
      }),
    ).toEqual({
      creditContainer: undefined,
      msaaSamples: 1,
      maxResolutionScale: 1.25,
      maximumRequestsPerServer: 6,
      tileCacheSize: 128,
      minimumZoomDistance: 75,
      maximumZoomDistance: 500_000,
    });

    const clamped = resolveGlobeSceneOptions({
      msaaSamples: 99,
      maxResolutionScale: 0.4,
      minimumZoomDistance: 5_000,
      maximumZoomDistance: 1_000,
    });
    expect(clamped.msaaSamples).toBe(4);
    expect(clamped.maxResolutionScale).toBe(1);
    expect(clamped.maximumZoomDistance).toBe(5_000);
  });

  it("caps resolution scale via maxResolutionScale while keeping floor >= 1", () => {
    expect(resolutionScaleForOptions(1.5, undefined, 3)).toBe(1.5);
    expect(resolutionScaleForOptions(1.5, 1.25, 3)).toBe(1.25);
    expect(resolutionScaleForOptions(1.5, 1.0, 3)).toBe(1.0);
    expect(resolutionScaleForOptions(1.5, 0.5, 3)).toBe(1.0);
    expect(resolutionScaleForOptions(1.5, 2.0, 1)).toBe(1.0);
  });

  it("steps quality governor down after 2 low ticks and up after 5 high ticks", () => {
    const changes: number[] = [];
    const gov = createQualityGovernor((level) => changes.push(level));
    expect(gov.getLevel()).toBe(0);

    expect(gov.step(40)).toBe(false);
    expect(gov.step(40)).toBe(true);
    expect(gov.getLevel()).toBe(1);

    // Streak resets on neutral fps
    expect(gov.step(40)).toBe(false);
    expect(gov.step(55)).toBe(false);
    expect(gov.step(40)).toBe(false);
    expect(gov.getLevel()).toBe(1);
    expect(gov.step(40)).toBe(true);
    expect(gov.getLevel()).toBe(QUALITY_LEVELS.length - 1);

    // Already at lowest quality
    expect(gov.step(20)).toBe(false);
    expect(gov.step(20)).toBe(false);
    expect(gov.getLevel()).toBe(QUALITY_LEVELS.length - 1);

    // 5 consecutive ticks > 57 fps step back up
    for (let i = 0; i < 4; i += 1) {
      expect(gov.step(60)).toBe(false);
    }
    expect(gov.step(60)).toBe(true);
    expect(gov.getLevel()).toBe(1);
    expect(changes).toEqual([1, 2, 1]);
  });

  it("viewer.ts delegates CesiumWidget bootstrap to shared globeScene module", () => {
    const viewerSrc = fs.readFileSync(path.join(root, "src/viz/viewer.ts"), "utf8");
    expect(viewerSrc).toContain('from "./globeScene"');
    expect(viewerSrc).toContain("createGlobeScene(");
    expect(viewerSrc).not.toContain("new CesiumWidget(");
    expect(viewerSrc).not.toContain("ArcGISTiledElevationTerrainProvider");
  });
});

describe("mobile touch controls, altitude conversion, and tail labels", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    resetGeoidForTests();
  });

  it("configures explicit mobile touch gestures and terrain collision safety", () => {
    const controller = {} as ScreenSpaceCameraController;
    configureMobileTouchControls(controller);
    expect(controller.enableInputs).toBe(true);
    expect(controller.enableRotate).toBe(true);
    expect(controller.enableZoom).toBe(true);
    expect(controller.enableTilt).toBe(true);
    expect(controller.enableLook).toBe(false);
    expect(controller.enableCollisionDetection).toBe(true);
    expect(controller.rotateEventTypes).toEqual([CameraEventType.LEFT_DRAG]);
    expect(controller.zoomEventTypes).toContain(CameraEventType.PINCH);
    expect(controller.tiltEventTypes).toContain(CameraEventType.PINCH);
    expect(controller.tiltEventTypes).toContain(CameraEventType.RIGHT_DRAG);
    expect(controller.lookEventTypes).toEqual([]);
    expect(controller.minimumZoomDistance).toBe(50);
    expect(controller.maximumZoomDistance).toBe(2_000_000);
    expect(controller.maximumTiltAngle).toBeLessThan(Math.PI / 2);
    expect(controller.inertiaSpin).toBe(MOBILE_TOUCH_INERTIA.spin);
    expect(controller.inertiaZoom).toBe(MOBILE_TOUCH_INERTIA.zoom);
  });

  it("converts MSL to WGS84 ellipsoid using EGM96 when loaded, falls back while loading, and maps null to 0", () => {
    resetGeoidForTests();
    expect(mslPointToEllipsoidAlt(-33.4, -70.57, null)).toBe(0);
    expect(mslPointToEllipsoidAlt(-33.4, -70.57, 1500)).toBe(1500);

    const meta = JSON.parse(
      readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
    ) as GeoidMeta;
    const bin = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));
    installGeoidGrid(meta, new Float32Array(bin.buffer, bin.byteOffset, bin.byteLength / 4));

    const n = geoidUndulationM(-33.4, -70.57);
    expect(mslToEllipsoid(-33.4, -70.57, 1500)).toBeCloseTo(1500 + n, 4);
    expect(mslPointToEllipsoidAlt(-33.4, -70.57, 1500)).toBeCloseTo(1500 + n, 4);
    expect(mslPointToEllipsoidAlt(-33.4, -70.57, null)).toBe(0);
  });

  it("fetches geoid assets with credentials: omit so no cookies are sent", async () => {
    resetGeoidForTests();
    const fetchMock = vi.fn(async () => ({
      ok: false,
      json: async () => ({}),
      arrayBuffer: async () => new ArrayBuffer(0),
    }));
    vi.stubGlobal("fetch", fetchMock);
    await ensureGeoidLoaded();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][1]).toEqual({ credentials: "omit" });
  });

  it("formats club vs pilot-entered tail labels using existing i18n keys in es and en", () => {
    initI18n({ stored: "es" });
    expect(formatAircraftLabelText("CC-ABC", "club")).toBe("CC-ABC");
    expect(formatAircraftLabelText("CC-ABC", "pilot")).toBe("CC-ABC · Registrada por el piloto");

    initI18n({ stored: "en" });
    expect(formatAircraftLabelText("CC-ABC", "club")).toBe("CC-ABC");
    expect(formatAircraftLabelText("CC-ABC", "pilot")).toBe("CC-ABC · Pilot-entered");
  });
});
