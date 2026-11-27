import {
  BoundingSphere,
  Cartesian2,
  Cartesian3,
  Cartographic,
  ClockRange,
  Color,
  ConstantPositionProperty,
  ConstantProperty,
  DistanceDisplayCondition,
  Entity,
  GeometryInstance,
  HeadingPitchRange,
  HeadingPitchRoll,
  HorizontalOrigin,
  JulianDate,
  LabelStyle,
  Math as CesiumMath,
  Matrix4,
  NearFarScalar,
  PointGraphics,
  PolylineColorAppearance,
  PolylineGeometry,
  Primitive,
  Transforms,
  VerticalOrigin,
  ScreenSpaceEventHandler,
  ScreenSpaceEventType,
} from "cesium";
import type { ColorMode, Flight, FlightTag, Sample, TrackPoint, TrimRange } from "../track/types";
import { headingRadians } from "../track/geo";
import { sampleAt } from "../track/parse";
import { fullRange, pointsInTrim } from "../track/trim";
import { colorForUnit, normalizeValue, valueForMode } from "./colors";
import { lerpAngle, wrapAngle, type FollowMode } from "./follow";
import { createGlobeScene } from "./globeScene";
import { computeTagLabelLayouts } from "./tagLayout";
import { e2eGlobeHooksEnabled, markGlobePainted } from "../e2eGlobeHooks";

export type FlightViewer = {
  setFlight: (flight: Flight, colorMode: ColorMode, trim?: TrimRange, fly?: boolean) => Promise<void>;
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
  getPerfStats: () => {
    commands: number;
    triangles: number;
    quality: number;
    farKm: number;
    tilesLoaded: boolean;
    resolutionScale: number;
  };
  stepQuality: (fps: number) => void;
  appendLivePoints: (newPoints: TrackPoint[]) => Promise<void>;
  setAccuracyDim: (thresholdM: number) => void;
  getAircraftPosition: () => { lon: number; lat: number; alt: number } | null;
  resize: () => void;
  destroy: () => void;
};

function pointAlt(p: TrackPoint): number {
  return p.positionAlt ?? p.alt;
}

/** Trace shows the whole path and the marker travels along it.
 *  Draw reveals the path progressively as the marker moves. */
export type PathMode = "trace" | "draw";

export type { FollowMode };

export async function createFlightViewer(container: HTMLElement): Promise<FlightViewer> {
  const globe = await createGlobeScene(container, {
    creditContainer: document.getElementById("credits") ?? undefined,
    onPostRender: e2eGlobeHooksEnabled() ? markGlobePainted : undefined,
  });
  const viewer = globe.widget;

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
  let accuracyThresholdM = Number.POSITIVE_INFINITY;

  function requestRender(): void {
    globe.requestRender();
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

  function stepQuality(fps: number): void {
    globe.stepQuality(fps);
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

  function colorWithAccuracy(base: Color, point: TrackPoint): Color {
    if ((point.accuracyM ?? 0) > accuracyThresholdM) {
      return Color.clone(base, new Color(base.red, base.green, base.blue, base.alpha * 0.32));
    }
    return base;
  }

  function captureFromCamera(sample: Sample): void {
    const target = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, pointAlt(sample.point));
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
      tagAnchors.set(tag.id, Cartesian3.fromDegrees(point.lon, point.lat, pointAlt(point) + 40));
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
    const anchors = shown
      .map((tag) => {
        const pos = tagAnchors.get(tag.id);
        return pos ? { id: tag.id, x: pos.x, y: pos.y, z: pos.z } : null;
      })
      .filter((a): a is { id: string; x: number; y: number; z: number } => a !== null);
    const placements = computeTagLabelLayouts(anchors);
    for (const tag of shown) {
      const position = tagAnchors.get(tag.id);
      if (!position) continue;
      const placement = placements.get(tag.id);
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
            font: "12px Outfit, sans-serif",
            fillColor: Color.WHITE,
            outlineColor: Color.BLACK,
            outlineWidth: 2,
            style: LabelStyle.FILL_AND_OUTLINE,
            showBackground: true,
            backgroundColor: Color.fromCssColorString("rgba(10,14,20,0.82)"),
            backgroundPadding: new Cartesian2(6, 4),
            pixelOffset: new Cartesian2(placement?.offsetX ?? 0, placement?.offsetY ?? -16),
            horizontalOrigin: HorizontalOrigin.CENTER,
            verticalOrigin:
              placement?.verticalOrigin === "TOP" ? VerticalOrigin.TOP : VerticalOrigin.BOTTOM,
            disableDepthTestDistance: Number.POSITIVE_INFINITY,
            scaleByDistance: new NearFarScalar(800, 1.0, 50_000, 0.6),
            translucencyByDistance: new NearFarScalar(15_000, 1.0, 75_000, 0.4),
            distanceDisplayCondition: new DistanceDisplayCondition(
              0,
              placement?.maxDisplayDistanceM ?? 75_000,
            ),
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
    linePositions = source.map((p) => Cartesian3.fromDegrees(p.lon, p.lat, pointAlt(p)));
    lineColors = source.map((p) =>
      colorWithAccuracy(colorForUnit(normalizeValue(next, mode, valueForMode(p, mode))), p),
    );
    allPositions = next.points.map((p) => Cartesian3.fromDegrees(p.lon, p.lat, pointAlt(p)));
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

  function applyCraftAccuracy(point: TrackPoint): void {
    if (!craft?.point) return;
    const dim = (point.accuracyM ?? 0) > accuracyThresholdM;
    craft.point.color = new ConstantProperty(
      dim ? Color.fromAlpha(Color.WHITE, 0.45) : Color.WHITE,
    );
    craft.point.outlineColor = new ConstantProperty(
      dim ? Color.fromAlpha(Color.fromCssColorString("#67e8f9"), 0.45) : Color.fromCssColorString("#67e8f9"),
    );
  }

  function setSample(sample: Sample, followCam: boolean): void {
    if (!flight || !craft) return;
    const position = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, pointAlt(sample.point));
    craft.position = new ConstantPositionProperty(position);
    applyCraftAccuracy(sample.point);
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
    const position = Cartesian3.fromDegrees(sample.point.lon, sample.point.lat, pointAlt(sample.point));
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
    const narrow = viewer.canvas.clientWidth > 0 && viewer.canvas.clientWidth < 640;
    const range = Math.max(sphere.radius * (narrow ? 2.5 : 2.1), narrow ? 9500 : 8000);
    viewer.camera.flyToBoundingSphere(sphere, {
      offset: new HeadingPitchRange(0, CesiumMath.toRadians(-48), range),
      duration: 1.2,
    });
    requestRender();
  }

  return {
    setFlight: async (next, mode, nextTrim, fly = true) => {
      following = false;
      captured = false;
      hasSmoothTarget = false;
      await rebuildTrack(next, mode, nextTrim ?? fullRange(next));
      if (fly) flyOverview();
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
        quality: globe.getQualityLevel(),
        farKm: rangeLimited ? rangeLimitM / 1000 : 0,
        tilesLoaded: viewer.scene.globe.tilesLoaded,
        resolutionScale: globe.getResolutionScale(),
      };
    },
    stepQuality,
    appendLivePoints: async (newPoints) => {
      if (!flight || !trim || newPoints.length === 0) return;
      const merged = [...flight.points, ...newPoints];
      const alts = merged.map((p) => pointAlt(p));
      const speeds = merged.map((p) => p.speedMps);
      const varios = merged.map((p) => p.varioMps);
      flight = {
        ...flight,
        points: merged,
        durationMs: merged[merged.length - 1].time - merged[0].time,
        distanceM: merged[merged.length - 1].distanceM,
        minAlt: Math.min(...alts),
        maxAlt: Math.max(...alts),
        maxSpeedMps: Math.max(...speeds),
        minVario: Math.min(...varios),
        maxVario: Math.max(...varios),
      };
      await rebuildTrack(flight, colorMode, trim);
    },
    setAccuracyDim: (thresholdM) => {
      accuracyThresholdM = thresholdM;
      if (!flight || !trim) return;
      void rebuildTrack(flight, colorMode, trim);
    },
    getAircraftPosition: () => {
      if (!craft?.position) return null;
      const cart = craft.position.getValue(viewer.clock.currentTime);
      if (!cart) return null;
      const carto = Cartographic.fromCartesian(cart);
      return {
        lon: CesiumMath.toDegrees(carto.longitude),
        lat: CesiumMath.toDegrees(carto.latitude),
        alt: carto.height,
      };
    },
    resize: () => {
      globe.resize();
    },
    destroy: () => {
      removeCameraMoveEnd();
      pickHandler.destroy();
      globe.destroy();
    },
  };
}
