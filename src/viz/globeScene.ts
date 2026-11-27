import {
  ArcGISTiledElevationTerrainProvider,
  CesiumWidget,
  Color,
  EllipsoidTerrainProvider,
  ImageryLayer,
  Ion,
  RequestScheduler,
  SceneMode,
  UrlTemplateImageryProvider,
  createWorldTerrainAsync,
} from "cesium";
import { QUALITY_LEVELS, resolutionScaleForLevel } from "./resolution";

export const ESRI_IMAGERY_URL =
  "https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}";
export const ESRI_TERRAIN_URL =
  "https://elevation3d.arcgis.com/arcgis/rest/services/WorldElevation3D/Terrain3D/ImageServer";
export const ESRI_CREDIT_TEXT = "Esri, Maxar, Earthstar Geographics";
export const HAZE_COLOR_HEX = "#a3b8c7";

export type GlobeSceneOptions = {
  creditContainer?: Element | string;
  msaaSamples?: number;
  maxResolutionScale?: number;
  maximumRequestsPerServer?: number;
  tileCacheSize?: number;
  minimumZoomDistance?: number;
  maximumZoomDistance?: number;
  onPostRender?: () => void;
};

export type ResolvedGlobeSceneOptions = {
  creditContainer: Element | string | undefined;
  msaaSamples: number;
  maxResolutionScale: number | undefined;
  maximumRequestsPerServer: number;
  tileCacheSize: number;
  minimumZoomDistance: number;
  maximumZoomDistance: number;
};

export const DEFAULT_GLOBE_SCENE_OPTIONS: Readonly<Omit<ResolvedGlobeSceneOptions, "creditContainer">> = {
  msaaSamples: 2,
  maxResolutionScale: undefined,
  maximumRequestsPerServer: 8,
  tileCacheSize: 256,
  minimumZoomDistance: 50,
  maximumZoomDistance: 2_000_000,
};

function clampInt(value: number | undefined, fallback: number, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.round(value)));
}

function clampFinite(value: number | undefined, fallback: number, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

export function resolveGlobeSceneOptions(opts?: GlobeSceneOptions): ResolvedGlobeSceneOptions {
  const minZoom = clampFinite(
    opts?.minimumZoomDistance,
    DEFAULT_GLOBE_SCENE_OPTIONS.minimumZoomDistance,
    1,
    100_000,
  );
  const maxZoom = Math.max(
    minZoom,
    clampFinite(
      opts?.maximumZoomDistance,
      DEFAULT_GLOBE_SCENE_OPTIONS.maximumZoomDistance,
      1_000,
      50_000_000,
    ),
  );
  const maxRes =
    typeof opts?.maxResolutionScale === "number" &&
    Number.isFinite(opts.maxResolutionScale) &&
    opts.maxResolutionScale > 0
      ? Math.max(1, opts.maxResolutionScale)
      : undefined;

  return {
    creditContainer: opts?.creditContainer,
    msaaSamples: clampInt(opts?.msaaSamples, DEFAULT_GLOBE_SCENE_OPTIONS.msaaSamples, 1, 4),
    maxResolutionScale: maxRes,
    maximumRequestsPerServer: clampInt(
      opts?.maximumRequestsPerServer,
      DEFAULT_GLOBE_SCENE_OPTIONS.maximumRequestsPerServer,
      1,
      18,
    ),
    tileCacheSize: clampInt(opts?.tileCacheSize, DEFAULT_GLOBE_SCENE_OPTIONS.tileCacheSize, 32, 1024),
    minimumZoomDistance: minZoom,
    maximumZoomDistance: maxZoom,
  };
}

export function resolutionScaleForOptions(
  pixelCap: number,
  maxResolutionScale?: number,
  devicePixelRatio?: number,
): number {
  const effectiveCap =
    typeof maxResolutionScale === "number" && Number.isFinite(maxResolutionScale) && maxResolutionScale > 0
      ? Math.min(pixelCap, Math.max(1, maxResolutionScale))
      : pixelCap;
  return resolutionScaleForLevel(effectiveCap, devicePixelRatio);
}

export type QualityGovernor = {
  step: (fps: number) => boolean;
  getLevel: () => number;
  current: () => (typeof QUALITY_LEVELS)[number];
};

export function createQualityGovernor(
  onChange?: (level: number, entry: (typeof QUALITY_LEVELS)[number]) => void,
): QualityGovernor {
  let qualityLevel = 0;
  let lowStreak = 0;
  let highStreak = 0;

  return {
    step: (fps: number) => {
      if (!Number.isFinite(fps)) return false;
      if (fps < 50 && qualityLevel < QUALITY_LEVELS.length - 1) {
        lowStreak += 1;
        highStreak = 0;
        if (lowStreak >= 2) {
          qualityLevel += 1;
          lowStreak = 0;
          onChange?.(qualityLevel, QUALITY_LEVELS[qualityLevel]);
          return true;
        }
        return false;
      }
      if (fps > 57 && qualityLevel > 0) {
        highStreak += 1;
        lowStreak = 0;
        if (highStreak >= 5) {
          qualityLevel -= 1;
          highStreak = 0;
          onChange?.(qualityLevel, QUALITY_LEVELS[qualityLevel]);
          return true;
        }
        return false;
      }
      lowStreak = 0;
      highStreak = 0;
      return false;
    },
    getLevel: () => qualityLevel,
    current: () => QUALITY_LEVELS[qualityLevel],
  };
}

export type GlobeSceneHandle = {
  widget: CesiumWidget;
  requestRender: () => void;
  applyQuality: () => void;
  stepQuality: (fps: number) => boolean;
  getQualityLevel: () => number;
  getResolutionScale: () => number;
  setPaused: (paused: boolean) => void;
  isPaused: () => boolean;
  resize: () => void;
  destroy: () => void;
};

export async function createGlobeScene(
  container: HTMLElement,
  opts?: GlobeSceneOptions,
): Promise<GlobeSceneHandle> {
  const resolved = resolveGlobeSceneOptions(opts);
  const ionToken = import.meta.env.VITE_CESIUM_ION_TOKEN as string | undefined;
  const useIonTerrain = import.meta.env.VITE_USE_CESIUM_ION_TERRAIN === "1";
  if (useIonTerrain && ionToken) Ion.defaultAccessToken = ionToken;

  const widget = new CesiumWidget(container, {
    creditContainer: resolved.creditContainer,
    terrainProvider: new EllipsoidTerrainProvider(),
    baseLayer: new ImageryLayer(
      new UrlTemplateImageryProvider({
        url: ESRI_IMAGERY_URL,
        credit: ESRI_CREDIT_TEXT,
        // Level 18 (~0.6 m/px) is plenty from flight altitudes and halves
        // the deepest tile count vs 19.
        maximumLevel: 18,
      }),
    ),
    sceneMode: SceneMode.SCENE3D,
    // Render on demand instead of every frame: the globe idles the GPU while
    // paused, and every mutation ends with an explicit requestRender().
    requestRenderMode: true,
    useBrowserRecommendedResolution: true,
  });

  const removePostRender = opts?.onPostRender
    ? widget.scene.postRender.addEventListener(() => {
        opts.onPostRender?.();
      })
    : undefined;

  widget.scene.globe.enableLighting = true;
  widget.scene.globe.depthTestAgainstTerrain = false;
  widget.scene.fog.enabled = true;
  // CesiumWidget adds a default Tycho-2 starfield skybox; with the far plane
  // clipping the atmosphere in follow mode the sky read as night. Drop the
  // stars (keep Sun/Moon/lighting) so the sky is always the haze backdrop.
  const defaultSkyBox = widget.scene.skyBox;
  if (defaultSkyBox) {
    widget.scene.skyBox = undefined;
    defaultSkyBox.destroy();
  }
  // Untextured/loading tiles show baseColor (default dark navy): keep it haze
  // so streaming terrain blooms in as fog instead of flashing dark.
  const hazeColor = Color.fromCssColorString(HAZE_COLOR_HEX);
  widget.scene.globe.baseColor = hazeColor;
  widget.scene.backgroundColor = hazeColor.clone();
  if (widget.scene.skyAtmosphere) widget.scene.skyAtmosphere.show = true;
  // Fill-rate and geometry budget: the globe is ~95% of every frame, so cap
  // pixel and terrain density where flight altitudes make it invisible.
  widget.scene.globe.maximumScreenSpaceError = QUALITY_LEVELS[0].sse;
  // 2x MSAA by default (configurable for mobile): lines stay smooth, fill halves.
  widget.scene.msaaSamples = resolved.msaaSamples;
  // Imagery diet for flaky links: fewer parallel fetches competing (default 18/server)
  // and a deeper tile cache so rotations reuse instead of refetching.
  RequestScheduler.maximumRequestsPerServer = resolved.maximumRequestsPerServer;
  widget.scene.globe.tileCacheSize = resolved.tileCacheSize;
  widget.scene.screenSpaceCameraController.minimumZoomDistance = resolved.minimumZoomDistance;
  widget.scene.screenSpaceCameraController.maximumZoomDistance = resolved.maximumZoomDistance;

  try {
    if (useIonTerrain && ionToken) {
      widget.terrainProvider = await createWorldTerrainAsync();
    } else {
      widget.terrainProvider = await ArcGISTiledElevationTerrainProvider.fromUrl(ESRI_TERRAIN_URL);
    }
  } catch (error) {
    console.warn("3D terrain unavailable, using ellipsoid", error);
  }

  let paused = false;

  function requestRender(): void {
    if (paused || widget.isDestroyed()) return;
    widget.scene.requestRender();
  }

  const governor = createQualityGovernor(() => {
    applyQuality();
  });

  function applyQuality(): void {
    if (widget.isDestroyed()) return;
    const q = governor.current();
    widget.resolutionScale = resolutionScaleForOptions(q.pixelCap, resolved.maxResolutionScale);
    widget.scene.globe.maximumScreenSpaceError = q.sse;
    widget.resize();
    requestRender();
  }

  applyQuality();

  return {
    widget,
    requestRender,
    applyQuality,
    stepQuality: (fps: number) => governor.step(fps),
    getQualityLevel: () => governor.getLevel(),
    getResolutionScale: () => widget.resolutionScale,
    setPaused: (nextPaused: boolean) => {
      if (widget.isDestroyed()) return;
      paused = nextPaused;
      widget.useDefaultRenderLoop = !paused;
      if (!paused) {
        widget.resize();
        widget.scene.requestRender();
      }
    },
    isPaused: () => paused,
    resize: () => {
      if (widget.isDestroyed()) return;
      widget.resize();
      requestRender();
    },
    destroy: () => {
      removePostRender?.();
      if (!widget.isDestroyed()) {
        widget.destroy();
      }
    },
  };
}
