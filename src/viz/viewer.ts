import {
  ArcGISTiledElevationTerrainProvider,
  BoundingSphere,
  Cartesian2,
  Cartesian3,
  ClockRange,
  Color,
  ConstantPositionProperty,
  ConstantProperty,
  EllipsoidTerrainProvider,
  Entity,
  GeometryInstance,
  HeadingPitchRange,
  HeadingPitchRoll,
  HorizontalOrigin,
  ImageryLayer,
  Ion,
  JulianDate,
  LabelStyle,
  Math as CesiumMath,
  Matrix4,
  NearFarScalar,
  PointGraphics,
  PolylineColorAppearance,
  PolylineGeometry,
  Primitive,
  RequestScheduler,
  SceneMode,
  Transforms,
  UrlTemplateImageryProvider,
  VerticalOrigin,
  ScreenSpaceEventHandler,
  ScreenSpaceEventType,
  Viewer,
  createWorldTerrainAsync,
} from "cesium";
import type { ColorMode, Flight, FlightTag, Sample, TrimRange } from "../track/types";
import { headingRadians } from "../track/geo";
import { sampleAt } from "../track/parse";
import { fullRange, pointsInTrim } from "../track/trim";
import { colorForUnit, normalizeValue, valueForMode } from "./colors";
import { lerpAngle, wrapAngle, type FollowMode } from "./follow";

const ESRI_IMAGERY =
  "https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}";
const ESRI_TERRAIN =
  "https://elevation3d.arcgis.com/arcgis/rest/services/WorldElevation3D/Terrain3D/ImageServer";

export type FlightViewer = {
  setFlight: (flight: Flight, colorMode: ColorMode, trim?: TrimRange) => Promise<void>;
  setColorMode: (mode: ColorMode) => Promise<void>;
  setTrim: (trim: TrimRange, fly?: boolean) => Promise<void>;
  setTags: (tags: FlightTag[]) => void;
  setSample: (sample: Sample, follow: boolean) => void;
  seekSample: (sample: Sample, follow: boolean) => void;
  setFollowMode: (mode: FollowMode) => void;
  captureFollowFromCamera: (sample: Sample) => void;
  unlockFollow: () => void;
  setOnTagPick: (handler: (timeMs: number) => void) => void;
  setPathMode: (mode: PathMode, timeMs: number) => void;
  flyOverview: () => void;
  getPerfStats: () => { commands: number; triangles: number; quality: number; farKm: number; tilesLoaded: boolean };
  stepQuality: (fps: number) => void;
  destroy: () => void;
};

/** Trace shows the whole path and the marker travels along it.
 *  Draw reveals the path progressively as the marker moves. */
export type PathMode = "trace" | "draw";

export type { FollowMode };

const QUALITY_LEVELS = [
  { pixelCap: 1.5, sse: 4 },
  { pixelCap: 1.25, sse: 6 },
  { pixelCap: 1.0, sse: 8 },
] as const;

export async function createFlightViewer(container: HTMLElement): Promise<FlightViewer> {
  const ionToken = import.meta.env.VITE_CESIUM_ION_TOKEN as string | undefined;
  if (ionToken) Ion.defaultAccessToken = ionToken;

  const viewer = new Viewer(container, {
    animation: false,
    timeline: false,
    baseLayerPicker: false,
    geocoder: false,
    homeButton: false,
    infoBox: false,
    sceneModePicker: false,
    selectionIndicator: false,
    navigationHelpButton: false,
    fullscreenButton: false,
    creditContainer: document.getElementById("credits") ?? undefined,
    terrainProvider: new EllipsoidTerrainProvider(),
    baseLayer: new ImageryLayer(
      new UrlTemplateImageryProvider({
        url: ESRI_IMAGERY,
        credit: "Esri, Maxar, Earthstar Geographics",
        // Level 18 (~0.6 m/px) is plenty from flight altitudes and halves
        // the deepest tile count vs 19.
        maximumLevel: 18,
      }),
    ),
    sceneMode: SceneMode.SCENE3D,
    // Render on demand instead of every frame: the globe idles the GPU while
    // paused, and every mutation below ends with an explicit requestRender().
    requestRenderMode: true,
  });

  viewer.scene.globe.enableLighting = true;
  viewer.scene.globe.depthTestAgainstTerrain = false;
  viewer.scene.fog.enabled = true;
  // The Viewer adds a default Tycho-2 starfield skybox; with the far plane
  // clipping the atmosphere in follow mode the sky read as night. Drop the
  // stars (keep Sun/Moon/lighting) so the sky is always the haze backdrop.
  const defaultSkyBox = viewer.scene.skyBox;
  if (defaultSkyBox) {
    viewer.scene.skyBox = undefined;
    defaultSkyBox.destroy();
  }
  // Untextured/loading tiles show baseColor (default dark navy): keep it haze
  // so streaming terrain blooms in as fog instead of flashing dark.
  viewer.scene.globe.baseColor = Color.fromCssColorString("#a3b8c7");
  if (viewer.scene.skyAtmosphere) viewer.scene.skyAtmosphere.show = true;
  // Fill-rate and geometry budget: the globe is ~95% of every frame, so cap
  // pixel and terrain density where flight altitudes make it invisible.
  // The auto quality governor below steps these down only while fps sags.
  // (Applied inline: applyQuality() reads closure state declared further down.)
  viewer.resolutionScale = Math.min(1, QUALITY_LEVELS[0].pixelCap / (window.devicePixelRatio || 1));
  viewer.scene.globe.maximumScreenSpaceError = QUALITY_LEVELS[0].sse;
  // 2x MSAA instead of 4x: lines stay smooth, multisample fill halves.
  viewer.scene.msaaSamples = 2;
  // Imagery diet for flaky links: failed tile textures render black, so make
  // failures rarer — fewer parallel fetches competing (default 18/server)
  // and a deeper tile cache so rotations reuse instead of refetching.
  RequestScheduler.maximumRequestsPerServer = 8;
  viewer.scene.globe.tileCacheSize = 256;
  viewer.scene.screenSpaceCameraController.minimumZoomDistance = 50;
  viewer.scene.screenSpaceCameraController.maximumZoomDistance = 2_000_000;

  try {
    if (ionToken) {
      viewer.terrainProvider = await createWorldTerrainAsync();
    } else {
      viewer.terrainProvider = await ArcGISTiledElevationTerrainProvider.fromUrl(ESRI_TERRAIN);
    }
  } catch (error) {
    console.warn("3D terrain unavailable, using ellipsoid", error);
  }

  let flight: Flight | null = null;
  let colorMode: ColorMode = "altitude";
  let trim: TrimRange | null = null;
  let tags: FlightTag[] = [];
  let pathMode: PathMode = "trace";
  let revealMs = Number.NEGATIVE_INFINITY;
  // Progressive reveal is append-only: the line is a list of fixed-grid chunk
  // primitives, so moving forward only builds new chunks instead of
  // re-uploading the whole polyline every frame.
  let trackChunks: Primitive[] = [];
  let drawnCount = 0;
  let lastPaintAt = 0;
  let drawnTagKey = "";
  let linePoints: Flight["points"] = [];
  let linePositions: Cartesian3[] = [];
  let lineColors: Color[] = [];
  // Full-flight positions for heading lookups (sample.index addresses
  // flight.points, while linePositions is trim-filtered).
  let allPositions: Cartesian3[] = [];
  // Tag anchor positions, resolved once per setTags/rebuild so the per-frame
  // reveal filter never scans the track.
  let tagAnchors = new Map<string, Cartesian3>();
  // Adaptive quality: full fidelity while the GPU keeps up, stepped down
  // only during sustained fps sags (camera motion on weak GPUs).
  let qualityLevel = 0;
  let lowStreak = 0;
  let highStreak = 0;
  let craft: Entity | undefined;
  let tagEntities: Entity[] = [];
  let trackPositions: Cartesian3[] = [];
  let followMode: FollowMode = "relative";
  let following = false;
  let captured = false;
  let worldHeading = 0;
  let relativeOffset = 0;
  let followPitch = CesiumMath.toRadians(-28);
  let followRange = 4200;
  let onTagPick: ((timeMs: number) => void) | undefined;
  let lastAppliedHeading = 0;
  let lastFollowMs = 0;
  let lastTrackHeading = 0;
  let smoothHeading: number | null = null;
  let smoothPitch: number | null = null;
  let smoothRange: number | null = null;
  const smoothTarget = new Cartesian3();
  let hasSmoothTarget = false;

  function requestRender(): void {
    viewer.scene.requestRender();
  }

  // Range limit: keep rendering bounded to ~10 km past the flight path.
  // Driven by camera distance to the current target, so it works for the
  // follow camera and a free camera alike: near the path the frustum far
  // plane clips everything past target + bubble, and horizon tiles are never
  // refined, loaded or drawn. Far away (overview) the default far plane
  // applies. Hysteresis keeps the switch from flickering at the boundary.
  // The backdrop is a constant haze tone so clipped regions and not-yet-
  // loaded tiles read as fog instead of flashing dark.
  const PATH_BUBBLE_M = 10_000;
  const NEAR_ENTER_M = 50_000;
  const NEAR_EXIT_M = 70_000;
  // Near/far live on the frustum (PerspectiveFrustum in 3D mode).
  const frustum = viewer.camera.frustum as unknown as { far: number };
  const farDefault = frustum.far;
  viewer.scene.backgroundColor = Color.fromCssColorString("#a3b8c7").clone();
  let rangeLimited = false;
  let rangeLimitM = 0;
  const lastTarget = new Cartesian3();
  let hasTarget = false;

  function marginFor(distMeters: number): number {
    return Math.max(5_000, distMeters * 0.3);
  }

  function updateRangeLimit(distMeters: number | null): void {
    if (distMeters == null) {
      if (!rangeLimited) return;
      rangeLimited = false;
      rangeLimitM = 0;
      frustum.far = farDefault;
      requestRender();
      return;
    }
    const near = rangeLimited ? distMeters < NEAR_EXIT_M : distMeters < NEAR_ENTER_M;
    if (!near) {
      if (!rangeLimited) return;
      rangeLimited = false;
      rangeLimitM = 0;
      frustum.far = farDefault;
      requestRender();
      return;
    }
    const far = distMeters + PATH_BUBBLE_M + marginFor(distMeters);
    rangeLimited = true;
    rangeLimitM = far;
    frustum.far = far;
  }

  function distToTarget(target: Cartesian3): number {
    return Cartesian3.distance(viewer.camera.positionWC, target);
  }

  function applyQuality(): void {
    const q = QUALITY_LEVELS[qualityLevel];
    viewer.resolutionScale = Math.min(1, q.pixelCap / (window.devicePixelRatio || 1));
    viewer.scene.globe.maximumScreenSpaceError = q.sse;
    requestRender();
  }

  function stepQuality(fps: number): void {
    if (fps < 50 && qualityLevel < QUALITY_LEVELS.length - 1) {
      lowStreak += 1;
      highStreak = 0;
      if (lowStreak >= 2) {
        qualityLevel += 1;
        lowStreak = 0;
        applyQuality();
      }
      return;
    }
    if (fps > 57 && qualityLevel > 0) {
      highStreak += 1;
      lowStreak = 0;
      if (highStreak >= 5) {
        qualityLevel -= 1;
        highStreak = 0;
        applyQuality();
      }
      return;
    }
    lowStreak = 0;
    highStreak = 0;
  }

  function trackHeading(sample: Sample): number {
    if (!flight) return 0;
    const later = sampleAt(flight, sample.t + 12_000);
    const a = allPositions[sample.index];
    const b = allPositions[later.index];
    if (!a || !b) return lastTrackHeading;
    const moved = headingRadians(
      sample.point.lat,
      sample.point.lon,
      later.point.lat,
      later.point.lon,
    );
    if (Cartesian3.distance(a, b) < 40) return lastTrackHeading;
    lastTrackHeading = moved;
    return moved;
  }

  function desiredHeading(sample: Sample): number {
    if (followMode === "fixed") return worldHeading;
    return wrapAngle(trackHeading(sample) + relativeOffset);
  }

  function captureFromCamera(sample: Sample): void {
    const target = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, sample.point.alt);
    worldHeading = viewer.camera.heading;
    relativeOffset = wrapAngle(worldHeading - trackHeading(sample));
    followPitch = viewer.camera.pitch;
    followRange = Math.max(600, Math.min(40_000, Cartesian3.distance(viewer.camera.positionWC, target)));
    lastAppliedHeading = worldHeading;
    captured = true;
    smoothHeading = worldHeading;
    smoothPitch = followPitch;
    smoothRange = followRange;
  }

  function applyFollowDefaults(): void {
    worldHeading = 0;
    relativeOffset = 0;
    followPitch = CesiumMath.toRadians(-28);
    followRange = 4200;
    captured = true;
    smoothHeading = null;
    smoothPitch = null;
    smoothRange = null;
  }

  function unlockCamera(): void {
    viewer.camera.lookAtTransform(Matrix4.IDENTITY);
  }

  function nearestPointIndex(timeMs: number): number {
    const pts = flight?.points ?? [];
    if (pts.length === 0) return -1;
    let lo = 0;
    let hi = pts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (pts[mid].time < timeMs) lo = mid + 1;
      else hi = mid;
    }
    const prev = Math.max(0, lo - 1);
    return Math.abs(pts[prev].time - timeMs) <= Math.abs(pts[lo].time - timeMs) ? prev : lo;
  }

  function resolveTagAnchors(): void {
    tagAnchors = new Map();
    if (!flight) return;
    for (const tag of tags) {
      const index = nearestPointIndex(tag.timeMs);
      if (index < 0) continue;
      const point = flight.points[index];
      tagAnchors.set(tag.id, Cartesian3.fromDegrees(point.lon, point.lat, point.alt + 40));
    }
  }

  function clearTags(): void {
    for (const entity of tagEntities) viewer.entities.remove(entity);
    tagEntities = [];
  }

  function drawTags(): void {
    const drawing = pathMode === "draw";
    const shown = drawing ? tags.filter((tag) => tag.timeMs <= revealMs) : tags;
    const key = shown.map((tag) => tag.id).join("\0");
    if (key === drawnTagKey) return;
    drawnTagKey = key;
    clearTags();
    if (!flight) return;
    for (const tag of shown) {
      const position = tagAnchors.get(tag.id);
      if (!position) continue;
      tagEntities.push(
        viewer.entities.add({
          id: `flight-tag:${tag.id}`,
          position,
          point: new PointGraphics({
            pixelSize: 10,
            color: Color.fromCssColorString("#fbbf24"),
            outlineColor: Color.WHITE,
            outlineWidth: 2,
            disableDepthTestDistance: Number.POSITIVE_INFINITY,
          }),
          label: {
            text: tag.label,
            font: "14px Outfit, sans-serif",
            fillColor: Color.WHITE,
            outlineColor: Color.BLACK,
            outlineWidth: 2,
            style: LabelStyle.FILL_AND_OUTLINE,
            showBackground: true,
            backgroundColor: Color.fromCssColorString("rgba(10,14,20,0.82)"),
            pixelOffset: new Cartesian2(0, -22),
            horizontalOrigin: HorizontalOrigin.CENTER,
            verticalOrigin: VerticalOrigin.BOTTOM,
            disableDepthTestDistance: Number.POSITIVE_INFINITY,
            scaleByDistance: new NearFarScalar(800, 1.05, 120000, 0.55),
          },
        }),
      );
    }
  }

  function countUntil(timeMs: number): number {
    const n = linePoints.length;
    if (n === 0) return 0;
    if (linePoints[0].time > timeMs) return 1;
    let lo = 0;
    let hi = n - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (linePoints[mid].time <= timeMs) lo = mid;
      else hi = mid - 1;
    }
    return lo + 1;
  }

  // Fixed-grid chunks share one joint point, so forward growth appends new
  // chunk primitives instead of re-uploading the whole polyline each frame.
  const REVEAL_CHUNK = 512;

  function addChunk(startIdx: number, endCount: number): void {
    if (endCount - startIdx < 2) return;
    const positions = linePositions.slice(startIdx, endCount);
    const colors = lineColors.slice(startIdx, endCount);
    if (positions.length < 2) return;
    trackChunks.push(
      viewer.scene.primitives.add(
        new Primitive({
          geometryInstances: new GeometryInstance({
            geometry: new PolylineGeometry({
              positions,
              width: 10,
              vertexFormat: PolylineColorAppearance.VERTEX_FORMAT,
              colors,
              colorsPerVertex: true,
            }),
          }),
          appearance: new PolylineColorAppearance({ translucent: false }),
          asynchronous: false,
        }),
      ),
    );
  }

  function clearChunks(): void {
    for (const chunk of trackChunks) viewer.scene.primitives.remove(chunk);
    trackChunks = [];
    drawnCount = 0;
  }

  function chunkIndexForCount(count: number): number {
    return Math.floor((count - 1) / REVEAL_CHUNK);
  }

  function appendTo(count: number): void {
    const total = linePositions.length;
    const target = Math.min(count, total);
    while (drawnCount < target) {
      // Overlap the joint point so consecutive chunks connect seamlessly.
      const start = drawnCount > 0 ? drawnCount - 1 : 0;
      const end = Math.min(target, chunkIndexForCount(drawnCount + 1) * REVEAL_CHUNK + REVEAL_CHUNK + 1);
      addChunk(start, end);
      drawnCount = end;
    }
    requestRender();
  }

  function truncateTo(count: number): void {
    const target = Math.max(0, count);
    if (target >= drawnCount) return;
    if (target < 2) {
      clearChunks();
      drawnCount = target;
      requestRender();
      return;
    }
    const keepChunk = Math.floor((target - 1) / (REVEAL_CHUNK + 1));
    while (trackChunks.length > keepChunk + 1) {
      viewer.scene.primitives.remove(trackChunks.pop()!);
    }
    viewer.scene.primitives.remove(trackChunks.pop()!);
    addChunk(keepChunk * REVEAL_CHUNK, target);
    drawnCount = target;
    requestRender();
  }

  function mountFull(): void {
    clearChunks();
    appendTo(linePositions.length);
  }

  function revealTo(timeMs: number, force = false): void {
    revealMs = timeMs;
    if (pathMode !== "draw") return;
    const count = countUntil(timeMs);
    if (count === drawnCount) return;
    const now = performance.now();
    const backward = count < drawnCount;
    if (!force && !backward && now - lastPaintAt < 80 && count - drawnCount < 12) return;
    lastPaintAt = now;
    if (backward) truncateTo(count);
    else appendTo(count);
    drawTags();
  }

  async function rebuildTrack(next: Flight, mode: ColorMode, nextTrim: TrimRange): Promise<void> {
    flight = next;
    colorMode = mode;
    trim = nextTrim;
    clearChunks();
    if (craft) {
      viewer.entities.remove(craft);
      craft = undefined;
    }

    const visible = pointsInTrim(next, nextTrim);
    const source = visible.length >= 2 ? visible : next.points;
    linePoints = source;
    linePositions = source.map((p) => Cartesian3.fromDegrees(p.lon, p.lat, p.alt));
    lineColors = source.map((p) => colorForUnit(normalizeValue(next, mode, valueForMode(p, mode))));
    allPositions = next.points.map((p) => Cartesian3.fromDegrees(p.lon, p.lat, p.alt));
    trackPositions = linePositions;
    revealMs = Math.min(Math.max(revealMs, source[0].time), source[source.length - 1].time);
    drawnTagKey = "";
    resolveTagAnchors();
    if (pathMode === "draw") appendTo(countUntil(revealMs));
    else mountFull();

    const start = JulianDate.fromDate(new Date(linePoints[0].time));
    const stop = JulianDate.fromDate(new Date(linePoints[linePoints.length - 1].time));
    viewer.clock.startTime = start;
    viewer.clock.stopTime = stop;
    viewer.clock.currentTime = JulianDate.clone(start);
    viewer.clock.clockRange = ClockRange.CLAMPED;
    viewer.clock.shouldAnimate = false;

    craft = viewer.entities.add({
      position: linePositions[0],
      point: new PointGraphics({
        pixelSize: 14,
        color: Color.WHITE,
        outlineColor: Color.fromCssColorString("#67e8f9"),
        outlineWidth: 3,
        disableDepthTestDistance: Number.POSITIVE_INFINITY,
      }),
    });
    drawTags();
  }

  function setSample(sample: Sample, followCam: boolean): void {
    if (!flight || !craft) return;
    const position = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, sample.point.alt);
    craft.position = new ConstantPositionProperty(position);
    viewer.clock.currentTime = JulianDate.fromDate(new Date(sample.t));
    const hpr = new HeadingPitchRoll(sample.point.headingRad, 0, 0);
    craft.orientation = new ConstantProperty(Transforms.headingPitchRollQuaternion(position, hpr));
    revealTo(sample.t);
    Cartesian3.clone(position, lastTarget);
    hasTarget = true;
    requestRender();

    if (!followCam) {
      if (following) unlockCamera();
      following = false;
      lastFollowMs = 0;
      updateRangeLimit(distToTarget(position));
      return;
    }

    if (!following) {
      if (!captured) applyFollowDefaults();
      following = true;
      lastFollowMs = 0;
      hasSmoothTarget = false;
      smoothHeading = null;
    }

    const now = performance.now();
    const dt = lastFollowMs ? Math.min(0.08, (now - lastFollowMs) / 1000) : 1 / 60;
    lastFollowMs = now;
    const ease = 1 - Math.exp(-dt / 0.32);

    const headingTarget = desiredHeading(sample);
    if (smoothHeading == null) smoothHeading = headingTarget;
    else smoothHeading = lerpAngle(smoothHeading, headingTarget, ease);
    if (smoothPitch == null) smoothPitch = followPitch;
    else smoothPitch += (followPitch - smoothPitch) * ease;
    if (smoothRange == null) smoothRange = followRange;
    else smoothRange += (followRange - smoothRange) * ease;

    if (!hasSmoothTarget) {
      Cartesian3.clone(position, smoothTarget);
      hasSmoothTarget = true;
    } else {
      Cartesian3.lerp(smoothTarget, position, ease, smoothTarget);
    }

    lastAppliedHeading = smoothHeading;
    viewer.camera.lookAt(
      smoothTarget,
      new HeadingPitchRange(smoothHeading, smoothPitch, smoothRange),
    );
    updateRangeLimit(distToTarget(smoothTarget));
    requestRender();
  }

  function seekSample(sample: Sample, followCam: boolean): void {
    hasSmoothTarget = false;
    lastFollowMs = 0;
    smoothHeading = null;
    smoothPitch = null;
    smoothRange = null;
    if (followCam) {
      setSample(sample, true);
      return;
    }
    setSample(sample, false);
    const position = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, sample.point.alt);
    unlockCamera();
    viewer.camera.flyToBoundingSphere(new BoundingSphere(position, 400), {
      offset: new HeadingPitchRange(sample.point.headingRad, CesiumMath.toRadians(-28), 3500),
      duration: 0.85,
    });
  }

  const pickHandler = new ScreenSpaceEventHandler(viewer.scene.canvas);
  pickHandler.setInputAction((movement: { position: Cartesian2 }) => {
    const picked = viewer.scene.pick(movement.position);
    const entity = picked?.id;
    if (!(entity instanceof Entity)) return;
    const id = String(entity.id);
    if (!id.startsWith("flight-tag:")) return;
    const tagId = id.slice("flight-tag:".length);
    const tag = tags.find((item) => item.id === tagId);
    if (tag) onTagPick?.(tag.timeMs);
  }, ScreenSpaceEventType.LEFT_CLICK);

  // Re-derive the limit after manual zoom/pan (no playhead updates then).
  const removeCameraMoveEnd = viewer.camera.moveEnd.addEventListener(() => {
    if (!hasTarget) return;
    updateRangeLimit(distToTarget(lastTarget));
  });

  function flyOverview(): void {
    if (!flight) return;
    updateRangeLimit(null);
    const drawing = pathMode === "draw";
    const pts = drawing ? linePositions.slice(0, Math.max(drawnCount, 0)) : trackPositions;
    if (pts.length < 2) {
      const pos = linePositions[Math.max(0, drawnCount - 1)] ?? linePositions[0];
      if (!pos) return;
      unlockCamera();
      viewer.camera.flyToBoundingSphere(new BoundingSphere(pos, 400), {
        offset: new HeadingPitchRange(0, CesiumMath.toRadians(-35), 4500),
        duration: 1.1,
      });
      return;
    }
    unlockCamera();
    const sphere = BoundingSphere.fromPoints(pts);
    viewer.camera.flyToBoundingSphere(sphere, {
      offset: new HeadingPitchRange(0, CesiumMath.toRadians(-48), Math.max(sphere.radius * 2.1, 8000)),
      duration: 1.2,
    });
    requestRender();
  }

  return {
    setFlight: async (next, mode, nextTrim) => {
      following = false;
      captured = false;
      hasSmoothTarget = false;
      await rebuildTrack(next, mode, nextTrim ?? fullRange(next));
      flyOverview();
    },
    setColorMode: async (mode) => {
      if (!flight || !trim) return;
      await rebuildTrack(flight, mode, trim);
    },
    setTrim: async (nextTrim, fly = false) => {
      if (!flight) return;
      await rebuildTrack(flight, colorMode, nextTrim);
      if (fly) flyOverview();
    },
    setTags: (next) => {
      tags = next;
      drawnTagKey = "";
      resolveTagAnchors();
      drawTags();
      requestRender();
    },
    setPathMode: (mode, timeMs) => {
      pathMode = mode;
      if (!linePositions.length) return;
      if (mode === "draw") revealTo(timeMs, true);
      else {
        mountFull();
        drawnTagKey = "";
        drawTags();
        requestRender();
      }
    },
    setSample,
    seekSample,
    setOnTagPick: (handler) => {
      onTagPick = handler;
    },
    setFollowMode: (mode) => {
      if (mode === followMode) return;
      followMode = mode;
      if (mode === "fixed") worldHeading = lastAppliedHeading;
      else relativeOffset = wrapAngle(lastAppliedHeading - lastTrackHeading);
    },
    captureFollowFromCamera: (sample) => {
      captureFromCamera(sample);
      following = false;
    },
    unlockFollow: () => {
      following = false;
      captured = false;
      lastFollowMs = 0;
      updateRangeLimit(null);
      unlockCamera();
    },
    flyOverview,
    getPerfStats: () => {
      const frameState = viewer.scene as unknown as {
        frameState?: { commandList?: Array<{ count?: unknown }> };
      };
      const commands = frameState.frameState?.commandList ?? [];
      let triangles = 0;
      for (const cmd of commands) {
        if (typeof cmd.count === "number") triangles += cmd.count / 3;
      }
      return {
        commands: commands.length,
        triangles: Math.round(triangles),
        quality: qualityLevel,
        farKm: rangeLimited ? rangeLimitM / 1000 : 0,
        tilesLoaded: viewer.scene.globe.tilesLoaded,
      };
    },
    stepQuality,
    destroy: () => {
      removeCameraMoveEnd();
      pickHandler.destroy();
      viewer.destroy();
    },
  };
}
