import {
  BoundingSphere,
  CameraEventType,
  Cartesian2,
  Cartesian3,
  Color,
  ConstantPositionProperty,
  ConstantProperty,
  DistanceDisplayCondition,
  Entity,
  GeometryInstance,
  HeadingPitchRange,
  HorizontalOrigin,
  LabelStyle,
  Math as CesiumMath,
  Matrix4,
  NearFarScalar,
  PointGraphics,
  PolylineColorAppearance,
  PolylineGeometry,
  Primitive,
  type ScreenSpaceCameraController,
  VerticalOrigin,
} from "cesium";
import { ensureGeoidLoading, isGeoidReady, mslToEllipsoid, onGeoidReady } from "../geo/geoid";
import { t } from "../i18n/index";
import { headingRadians } from "../track/geo";
import { colorForUnit } from "./colors";
import { lerpAngle } from "./follow";
import { createGlobeScene, type GlobeSceneOptions } from "./globeScene";
import { attachGestureFollowBreaker } from "./touchGestures";
import {
  applyFlightsUpdate,
  type BridgeLayer,
  type BridgePoint,
  type BridgeTailSource,
  type BridgeUnits,
  type FlightsMessage,
  type StoredFlight,
} from "../embed/bridge";

const CHUNK_SIZE = 512;
const POLYLINE_REBUILD_THROTTLE_MS = 1_000;
const MIN_CAMERA_AGL_M = 20;
const FOLLOW_PITCH_RAD = CesiumMath.toRadians(-28);
const FOLLOW_RANGE_M = 4_200;

export const MOBILE_TOUCH_INERTIA = Object.freeze({
  spin: 0.82,
  translate: 0.82,
  zoom: 0.7,
});

export function configureMobileTouchControls(controller: ScreenSpaceCameraController): void {
  controller.enableInputs = true;
  controller.enableRotate = true;
  controller.enableZoom = true;
  controller.enableTilt = true;
  controller.enableLook = false;
  controller.enableCollisionDetection = true;
  // One-finger drag = rotate/orbit-pan the 3D globe; pinch = zoom;
  // two-finger vertical drag / right-drag = tilt; two-finger twist = heading.
  controller.rotateEventTypes = [CameraEventType.LEFT_DRAG];
  controller.zoomEventTypes = [CameraEventType.PINCH, CameraEventType.WHEEL];
  controller.tiltEventTypes = [
    CameraEventType.PINCH,
    CameraEventType.RIGHT_DRAG,
    CameraEventType.MIDDLE_DRAG,
  ];
  controller.lookEventTypes = [];
  controller.minimumZoomDistance = 50;
  controller.maximumZoomDistance = 2_000_000;
  controller.maximumTiltAngle = Math.PI / 2 - 0.05;
  controller.inertiaSpin = MOBILE_TOUCH_INERTIA.spin;
  controller.inertiaTranslate = MOBILE_TOUCH_INERTIA.translate;
  controller.inertiaZoom = MOBILE_TOUCH_INERTIA.zoom;
}

/**
 * Converts an incoming `[tMs, lat, lon, altMslM|null]` altitude to WGS84 ellipsoid height
 * for Cesium placement, mirroring `fixesToFlight.ts`:
 * - `alt === null` -> `0` m
 * - geoid ready -> `altMslM + geoidUndulationM(lat, lon)` (MSL -> WGS84 ellipsoid)
 * - geoid loading -> temporary raw `altMslM`, rebuilt once `onGeoidReady` fires.
 */
export function mslPointToEllipsoidAlt(
  lat: number,
  lon: number,
  altMslM: number | null,
): number {
  if (altMslM == null || !Number.isFinite(altMslM)) return 0;
  if (!isGeoidReady()) return altMslM;
  return mslToEllipsoid(lat, lon, altMslM);
}

/**
 * Plain-text tail label rendered by Cesium (never HTML). Reuses `common.pilotEntered`
 * so free-text pilot tails match the 2D public map's disambiguation badge.
 */
export function formatAircraftLabelText(tail: string, tailSource: BridgeTailSource): string {
  if (tailSource === "pilot") {
    return `${tail} · ${t("common.pilotEntered")}`;
  }
  return tail;
}

function pointAltitudeColor(altMslM: number | null, minAlt: number, maxAlt: number): Color {
  const val = altMslM ?? 0;
  const span = Math.max(1, maxAlt - minAlt);
  return colorForUnit((val - minAlt) / span);
}

function computeAltBounds(points: readonly BridgePoint[]): { minAlt: number; maxAlt: number } {
  let minAlt = Number.POSITIVE_INFINITY;
  let maxAlt = Number.NEGATIVE_INFINITY;
  for (const pt of points) {
    const a = pt[3] ?? 0;
    if (a < minAlt) minAlt = a;
    if (a > maxAlt) maxAlt = a;
  }
  if (!Number.isFinite(minAlt) || !Number.isFinite(maxAlt)) {
    return { minAlt: 0, maxAlt: 0 };
  }
  return { minAlt, maxAlt };
}

function widthForFlight(isOwn: boolean, layer: BridgeLayer): number {
  if (isOwn || layer === "own") return 10;
  if (layer === "club") return 6;
  return 5;
}

function markerStyle(isOwn: boolean, layer: BridgeLayer): {
  pixelSize: number;
  color: Color;
  outlineColor: Color;
  outlineWidth: number;
} {
  if (isOwn || layer === "own") {
    return {
      pixelSize: 16,
      color: Color.WHITE,
      outlineColor: Color.fromCssColorString("#67e8f9"),
      outlineWidth: 4,
    };
  }
  if (layer === "club") {
    return {
      pixelSize: 12,
      color: Color.WHITE,
      outlineColor: Color.fromCssColorString("#38bdf8"),
      outlineWidth: 2.5,
    };
  }
  return {
    pixelSize: 11,
    color: Color.fromCssColorString("#f8fafc"),
    outlineColor: Color.fromCssColorString("#cbd5e1"),
    outlineWidth: 2,
  };
}

function labelStyleForFlight(isOwn: boolean, tailSource: BridgeTailSource): {
  fillColor: Color;
  backgroundColor: Color;
} {
  if (tailSource === "pilot") {
    return {
      fillColor: Color.fromCssColorString("#fde68a"),
      backgroundColor: Color.fromCssColorString("rgba(69, 26, 3, 0.88)"),
    };
  }
  if (isOwn) {
    return {
      fillColor: Color.fromCssColorString("#ecfeff"),
      backgroundColor: Color.fromCssColorString("rgba(8, 51, 68, 0.9)"),
    };
  }
  return {
    fillColor: Color.WHITE,
    backgroundColor: Color.fromCssColorString("rgba(10, 14, 20, 0.82)"),
  };
}

function recentHeadingRad(points: readonly BridgePoint[], fallback: number): number {
  if (points.length < 2) return fallback;
  const last = points[points.length - 1];
  for (let i = points.length - 2; i >= 0; i -= 1) {
    const prev = points[i];
    const dLat = (last[1] - prev[1]) * 111_132;
    const dLon = (last[2] - prev[2]) * 111_132 * Math.cos((last[1] * Math.PI) / 180);
    if (Math.hypot(dLat, dLon) >= 25) {
      return headingRadians(prev[1], prev[2], last[1], last[2]);
    }
  }
  return fallback;
}

export type MultiSceneCallbacks = {
  onFollowChanged?: (event: { on: false; reason: "gesture" }) => void;
  onPerf?: (quality: number) => void;
  onPainted?: () => void;
};

export type MultiSceneOptions = GlobeSceneOptions & MultiSceneCallbacks;

export type MultiSceneHandle = {
  applyFlights: (msg: FlightsMessage) => void;
  setOwnFlightId: (id: string | null) => void;
  setUnits: (units: BridgeUnits) => void;
  refreshLocale: () => void;
  setFollow: (on: boolean, flightId?: string) => void;
  recenter: () => void;
  overview: () => void;
  setPaused: (paused: boolean) => void;
  isFollowing: () => boolean;
  isPainted: () => boolean;
  flightCount: () => number;
  pointCount: (id: string) => number;
  getQualityLevel: () => number;
  resize: () => void;
  destroy: () => void;
};

type FlightVisual = {
  entity: Entity;
  chunks: Primitive[];
  positions: Cartesian3[];
  colors: Color[];
  drawnCount: number;
  minAlt: number;
  maxAlt: number;
  width: number;
  lastPolylineAt: number;
  dirtyPolyline: boolean;
};

export async function createMultiScene(
  container: HTMLElement,
  opts: MultiSceneOptions = {},
): Promise<MultiSceneHandle> {
  let painted = false;
  const globe = await createGlobeScene(container, {
    creditContainer: opts.creditContainer,
    msaaSamples: opts.msaaSamples ?? 2,
    maxResolutionScale: opts.maxResolutionScale ?? 1.5,
    maximumRequestsPerServer: opts.maximumRequestsPerServer,
    tileCacheSize: opts.tileCacheSize,
    minimumZoomDistance: opts.minimumZoomDistance,
    maximumZoomDistance: opts.maximumZoomDistance,
    onPostRender: () => {
      if (!painted) {
        painted = true;
        opts.onPainted?.();
      }
      opts.onPostRender?.();
    },
  });

  const widget = globe.widget;
  configureMobileTouchControls(widget.scene.screenSpaceCameraController);

  // Clamp camera height above local terrain so touch tilts/zooms never clip underground.
  const removeTerrainClamp = widget.scene.preRender.addEventListener(() => {
    if (widget.isDestroyed()) return;
    const carto = widget.camera.positionCartographic;
    if (!carto) return;
    const terrainHeight = widget.scene.globe.getHeight(carto);
    const minHeight =
      (terrainHeight !== undefined && Number.isFinite(terrainHeight) ? terrainHeight : 0) +
      MIN_CAMERA_AGL_M;
    if (carto.height < minHeight) {
      widget.camera.position = Cartesian3.fromRadians(carto.longitude, carto.latitude, minHeight);
    }
  });

  let flights = new Map<string, StoredFlight>();
  const visuals = new Map<string, FlightVisual>();
  let ownFlightId: string | null = null;
  let _units: BridgeUnits = "m";
  let following = false;
  let followFlightId: string | null = null;
  let hasInitialFraming = false;
  let paused = false;
  let destroyed = false;
  let interacting = false;

  // Smooth chase camera state (reusing follow.ts angle interpolation).
  let smoothHeading: number | null = null;
  let smoothPitch: number | null = null;
  let smoothRange: number | null = null;
  const smoothTarget = new Cartesian3();
  let hasSmoothTarget = false;
  let lastFollowMs = 0;
  let lastTrackHeading = 0;

  // Throttled polyline rebuild timer (at most 1x/s during high-rate deltas).
  let polylineTimer: ReturnType<typeof setTimeout> | null = null;

  // FPS quality governor loop (runs only while interacting or following, stops when paused).
  let rafId: number | null = null;
  let qualityFrames = 0;
  let qualityWindowStart = 0;

  function isOwnFlight(flight: StoredFlight): boolean {
    if (ownFlightId !== null && flight.id === ownFlightId) return true;
    return flight.layer === "own";
  }

  function resolveOwnFlight(): StoredFlight | null {
    if (ownFlightId) {
      const explicit = flights.get(ownFlightId);
      if (explicit) return explicit;
    }
    for (const f of flights.values()) {
      if (f.layer === "own") return f;
    }
    return null;
  }

  function resolveFollowFlight(): StoredFlight | null {
    if (followFlightId) {
      const target = flights.get(followFlightId);
      if (target) return target;
    }
    return resolveOwnFlight();
  }

  function pointToCartesian(pt: BridgePoint): Cartesian3 {
    const h = mslPointToEllipsoidAlt(pt[1], pt[2], pt[3]);
    return Cartesian3.fromDegrees(pt[2], pt[1], h);
  }

  function clearVisualChunks(visual: FlightVisual): void {
    for (const chunk of visual.chunks) {
      widget.scene.primitives.remove(chunk);
    }
    visual.chunks = [];
    visual.drawnCount = 0;
  }

  function addPolylineChunk(
    visual: FlightVisual,
    startIdx: number,
    endCount: number,
    width: number,
  ): void {
    if (endCount - startIdx < 2) return;
    const positions = visual.positions.slice(startIdx, endCount);
    const colors = visual.colors.slice(startIdx, endCount);
    if (positions.length < 2) return;
    const primitive = widget.scene.primitives.add(
      new Primitive({
        geometryInstances: new GeometryInstance({
          geometry: new PolylineGeometry({
            positions,
            width,
            vertexFormat: PolylineColorAppearance.VERTEX_FORMAT,
            colors,
            colorsPerVertex: true,
          }),
        }),
        appearance: new PolylineColorAppearance({ translucent: false }),
        asynchronous: false,
      }),
    );
    visual.chunks.push(primitive);
  }

  function appendVisualChunks(visual: FlightVisual, width: number): void {
    const total = visual.positions.length;
    if (total < 2) {
      clearVisualChunks(visual);
      visual.drawnCount = total;
      return;
    }
    // If the trailing chunk is partial (< CHUNK_SIZE segments), replace only that trailing chunk.
    if (visual.drawnCount > 0 && visual.chunks.length > 0) {
      const prevChunkIdx = Math.floor((visual.drawnCount - 1) / CHUNK_SIZE);
      const prevChunkEnd = prevChunkIdx * CHUNK_SIZE + CHUNK_SIZE + 1;
      if (visual.drawnCount < prevChunkEnd) {
        const popped = visual.chunks.pop();
        if (popped) widget.scene.primitives.remove(popped);
        visual.drawnCount = prevChunkIdx * CHUNK_SIZE;
      }
    }
    while (visual.drawnCount < total) {
      const start = visual.drawnCount > 0 ? visual.drawnCount - 1 : 0;
      const chunkIdx = Math.floor(start / CHUNK_SIZE);
      const end = Math.min(total, chunkIdx * CHUNK_SIZE + CHUNK_SIZE + 1);
      addPolylineChunk(visual, start, end, width);
      visual.drawnCount = end;
    }
  }

  function syncVisualPolyline(
    flight: StoredFlight,
    visual: FlightVisual,
    appendedStrictly: boolean,
  ): void {
    const isOwn = isOwnFlight(flight);
    const width = widthForFlight(isOwn, flight.layer);
    const bounds = computeAltBounds(flight.points);
    const altBoundsUnchanged =
      visual.drawnCount > 0 &&
      Math.abs(bounds.minAlt - visual.minAlt) < 1 &&
      Math.abs(bounds.maxAlt - visual.maxAlt) < 1 &&
      visual.width === width;

    if (
      appendedStrictly &&
      altBoundsUnchanged &&
      flight.points.length >= visual.positions.length
    ) {
      for (let i = visual.positions.length; i < flight.points.length; i += 1) {
        const pt = flight.points[i];
        visual.positions.push(pointToCartesian(pt));
        visual.colors.push(pointAltitudeColor(pt[3], visual.minAlt, visual.maxAlt));
      }
      appendVisualChunks(visual, width);
    } else {
      clearVisualChunks(visual);
      visual.minAlt = bounds.minAlt;
      visual.maxAlt = bounds.maxAlt;
      visual.width = width;
      visual.positions = flight.points.map(pointToCartesian);
      visual.colors = flight.points.map((pt) =>
        pointAltitudeColor(pt[3], bounds.minAlt, bounds.maxAlt),
      );
      appendVisualChunks(visual, width);
    }
    visual.lastPolylineAt = performance.now();
    visual.dirtyPolyline = false;
  }

  function schedulePolylineFlush(): void {
    if (polylineTimer != null || paused || destroyed) return;
    polylineTimer = setTimeout(() => {
      polylineTimer = null;
      if (paused || destroyed) return;
      let flushed = false;
      for (const [id, visual] of visuals.entries()) {
        if (!visual.dirtyPolyline) continue;
        const flight = flights.get(id);
        if (!flight) continue;
        syncVisualPolyline(flight, visual, false);
        flushed = true;
      }
      if (flushed) globe.requestRender();
    }, POLYLINE_REBUILD_THROTTLE_MS);
  }

  function upsertFlightVisual(
    flight: StoredFlight,
    immediatePolyline: boolean,
    appendedStrictly: boolean,
  ): void {
    const isOwn = isOwnFlight(flight);
    const latestPt = flight.points[flight.points.length - 1];
    const latestPos = latestPt ? pointToCartesian(latestPt) : undefined;
    const marker = markerStyle(isOwn, flight.layer);
    const lblStyle = labelStyleForFlight(isOwn, flight.tailSource);
    const labelText = formatAircraftLabelText(flight.tail, flight.tailSource);

    let visual = visuals.get(flight.id);
    if (!visual) {
      const entity = widget.entities.add({
        id: `embed-flight:${flight.id}`,
        position: latestPos,
        show: Boolean(latestPos),
        point: new PointGraphics({
          pixelSize: marker.pixelSize,
          color: marker.color,
          outlineColor: marker.outlineColor,
          outlineWidth: marker.outlineWidth,
          disableDepthTestDistance: Number.POSITIVE_INFINITY,
        }),
        label: {
          text: labelText,
          font: isOwn ? "600 13px system-ui, sans-serif" : "500 12px system-ui, sans-serif",
          fillColor: lblStyle.fillColor,
          outlineColor: Color.BLACK,
          outlineWidth: 2,
          style: LabelStyle.FILL_AND_OUTLINE,
          showBackground: true,
          backgroundColor: lblStyle.backgroundColor,
          backgroundPadding: new Cartesian2(7, 4),
          pixelOffset: new Cartesian2(0, -18),
          horizontalOrigin: HorizontalOrigin.CENTER,
          verticalOrigin: VerticalOrigin.BOTTOM,
          disableDepthTestDistance: Number.POSITIVE_INFINITY,
          scaleByDistance: new NearFarScalar(1_000, 1.0, 80_000, 0.65),
          distanceDisplayCondition: new DistanceDisplayCondition(0, 250_000),
        },
      });
      visual = {
        entity,
        chunks: [],
        positions: [],
        colors: [],
        drawnCount: 0,
        minAlt: 0,
        maxAlt: 0,
        width: widthForFlight(isOwn, flight.layer),
        lastPolylineAt: 0,
        dirtyPolyline: false,
      };
      visuals.set(flight.id, visual);
    } else {
      visual.entity.show = Boolean(latestPos);
      if (latestPos) {
        visual.entity.position = new ConstantPositionProperty(latestPos);
      }
      if (visual.entity.point) {
        visual.entity.point.pixelSize = new ConstantProperty(marker.pixelSize);
        visual.entity.point.color = new ConstantProperty(marker.color);
        visual.entity.point.outlineColor = new ConstantProperty(marker.outlineColor);
        visual.entity.point.outlineWidth = new ConstantProperty(marker.outlineWidth);
      }
      if (visual.entity.label) {
        visual.entity.label.text = new ConstantProperty(labelText);
        visual.entity.label.fillColor = new ConstantProperty(lblStyle.fillColor);
        visual.entity.label.backgroundColor = new ConstantProperty(lblStyle.backgroundColor);
      }
    }

    const now = performance.now();
    if (
      immediatePolyline ||
      visual.lastPolylineAt === 0 ||
      now - visual.lastPolylineAt >= POLYLINE_REBUILD_THROTTLE_MS
    ) {
      syncVisualPolyline(flight, visual, appendedStrictly);
    } else {
      visual.dirtyPolyline = true;
      schedulePolylineFlush();
    }
  }

  function removeFlightVisual(id: string): void {
    const visual = visuals.get(id);
    if (!visual) return;
    clearVisualChunks(visual);
    widget.entities.remove(visual.entity);
    visuals.delete(id);
  }

  function rebuildAllPositionsForGeoid(): void {
    if (destroyed) return;
    for (const [id, flight] of flights.entries()) {
      const visual = visuals.get(id);
      if (!visual) continue;
      upsertFlightVisual(flight, true, false);
    }
    if (following) stepFollowCamera();
    globe.requestRender();
  }

  const offGeoidReady = onGeoidReady(() => {
    if (flights.size > 0) rebuildAllPositionsForGeoid();
  });

  function unlockCamera(): void {
    if (widget.isDestroyed()) return;
    widget.camera.lookAtTransform(Matrix4.IDENTITY);
  }

  function resetSmoothFollow(): void {
    hasSmoothTarget = false;
    smoothHeading = null;
    smoothPitch = null;
    smoothRange = null;
    lastFollowMs = 0;
  }

  function stepFollowCamera(): void {
    if (!following || paused || destroyed) return;
    const targetFlight = resolveFollowFlight();
    if (!targetFlight || targetFlight.points.length === 0) return;

    const latest = targetFlight.points[targetFlight.points.length - 1];
    const targetPos = pointToCartesian(latest);
    const headingTarget = recentHeadingRad(targetFlight.points, lastTrackHeading);
    lastTrackHeading = headingTarget;

    const now = performance.now();
    const dt = lastFollowMs ? Math.min(0.08, (now - lastFollowMs) / 1000) : 1 / 60;
    lastFollowMs = now;
    const ease = 1 - Math.exp(-dt / 0.32);

    if (smoothHeading == null) smoothHeading = headingTarget;
    else smoothHeading = lerpAngle(smoothHeading, headingTarget, ease);
    if (smoothPitch == null) smoothPitch = FOLLOW_PITCH_RAD;
    else smoothPitch += (FOLLOW_PITCH_RAD - smoothPitch) * ease;
    if (smoothRange == null) smoothRange = FOLLOW_RANGE_M;
    else smoothRange += (FOLLOW_RANGE_M - smoothRange) * ease;

    if (!hasSmoothTarget) {
      Cartesian3.clone(targetPos, smoothTarget);
      hasSmoothTarget = true;
    } else {
      Cartesian3.lerp(smoothTarget, targetPos, ease, smoothTarget);
    }

    widget.camera.lookAt(
      smoothTarget,
      new HeadingPitchRange(smoothHeading, smoothPitch, smoothRange),
    );
    globe.requestRender();
  }

  function breakFollowByGesture(): void {
    if (!following) return;
    following = false;
    resetSmoothFollow();
    unlockCamera();
    globe.requestRender();
    opts.onFollowChanged?.({ on: false, reason: "gesture" });
  }

  const detachGestures = attachGestureFollowBreaker(
    container,
    () => following,
    breakFollowByGesture,
    (active) => {
      interacting = active;
      if (active && !paused && rafId == null) startTickLoop();
    },
  );

  function collectAllPositions(): Cartesian3[] {
    const out: Cartesian3[] = [];
    for (const flight of flights.values()) {
      for (const pt of flight.points) {
        out.push(pointToCartesian(pt));
      }
    }
    return out;
  }

  function overview(): void {
    if (destroyed) return;
    following = false;
    resetSmoothFollow();
    unlockCamera();
    const pts = collectAllPositions();
    if (pts.length === 0) return;
    hasInitialFraming = true;
    if (pts.length === 1) {
      widget.camera.flyToBoundingSphere(new BoundingSphere(pts[0], 400), {
        offset: new HeadingPitchRange(0, CesiumMath.toRadians(-35), 4_500),
        duration: 0.9,
      });
      globe.requestRender();
      return;
    }
    const sphere = BoundingSphere.fromPoints(pts);
    const range = Math.max(sphere.radius * 2.4, 8_500);
    widget.camera.flyToBoundingSphere(sphere, {
      offset: new HeadingPitchRange(0, CesiumMath.toRadians(-48), range),
      duration: 1.0,
    });
    globe.requestRender();
  }

  function recenter(): void {
    if (destroyed) return;
    const own = resolveOwnFlight();
    if (own && own.points.length > 0) {
      hasInitialFraming = true;
      const latest = own.points[own.points.length - 1];
      const pos = pointToCartesian(latest);
      const heading = recentHeadingRad(own.points, lastTrackHeading);
      lastTrackHeading = heading;
      if (following) {
        resetSmoothFollow();
        stepFollowCamera();
      } else {
        unlockCamera();
        widget.camera.flyToBoundingSphere(new BoundingSphere(pos, 400), {
          offset: new HeadingPitchRange(heading, FOLLOW_PITCH_RAD, FOLLOW_RANGE_M),
          duration: 0.9,
        });
        globe.requestRender();
      }
      return;
    }
    overview();
  }

  function tick(now: number): void {
    rafId = null;
    if (destroyed || paused) return;
    if (following) {
      stepFollowCamera();
    }
    if (interacting || following) {
      if (!qualityWindowStart) qualityWindowStart = now;
      qualityFrames += 1;
      if (now - qualityWindowStart >= 2_000) {
        const fps = (qualityFrames * 1000) / Math.max(1, now - qualityWindowStart);
        globe.stepQuality(fps);
        opts.onPerf?.(globe.getQualityLevel());
        qualityFrames = 0;
        qualityWindowStart = now;
      }
      rafId = requestAnimationFrame(tick);
    } else {
      qualityFrames = 0;
      qualityWindowStart = 0;
    }
  }

  function startTickLoop(): void {
    if (destroyed || paused || rafId != null) return;
    rafId = requestAnimationFrame(tick);
  }

  function stopTickLoop(): void {
    if (rafId != null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    qualityFrames = 0;
    qualityWindowStart = 0;
  }

  ensureGeoidLoading();

  return {
    applyFlights: (msg: FlightsMessage) => {
      if (destroyed) return;
      ensureGeoidLoading();
      const diff = applyFlightsUpdate(flights, msg);
      flights = diff.flights;

      for (const id of diff.removed) {
        removeFlightVisual(id);
      }

      const immediate = msg.mode === "full";
      for (const id of diff.added) {
        const f = flights.get(id);
        if (f) upsertFlightVisual(f, true, false);
      }
      for (const id of diff.updated) {
        const f = flights.get(id);
        if (f) upsertFlightVisual(f, immediate, diff.appendedStrictly.has(id));
      }

      if (following) {
        stepFollowCamera();
        startTickLoop();
      } else if (!hasInitialFraming && flights.size > 0) {
        recenter();
      } else {
        globe.requestRender();
      }
    },
    setOwnFlightId: (id: string | null) => {
      if (ownFlightId === id) return;
      ownFlightId = id;
      for (const flight of flights.values()) {
        upsertFlightVisual(flight, true, false);
      }
      if (following) stepFollowCamera();
      globe.requestRender();
    },
    setUnits: (units: BridgeUnits) => {
      _units = units;
      void _units;
    },
    refreshLocale: () => {
      if (destroyed) return;
      for (const [id, visual] of visuals.entries()) {
        const flight = flights.get(id);
        if (flight && visual.entity.label) {
          visual.entity.label.text = new ConstantProperty(
            formatAircraftLabelText(flight.tail, flight.tailSource),
          );
        }
      }
      globe.requestRender();
    },
    setFollow: (on: boolean, flightId?: string) => {
      if (destroyed) return;
      if (flightId !== undefined) {
        followFlightId = flightId;
      }
      if (!on) {
        following = false;
        resetSmoothFollow();
        unlockCamera();
        globe.requestRender();
        return;
      }
      following = true;
      resetSmoothFollow();
      stepFollowCamera();
      startTickLoop();
    },
    recenter,
    overview,
    setPaused: (nextPaused: boolean) => {
      if (destroyed || paused === nextPaused) return;
      paused = nextPaused;
      if (paused) {
        stopTickLoop();
        if (polylineTimer != null) {
          clearTimeout(polylineTimer);
          polylineTimer = null;
        }
      }
      globe.setPaused(paused);
      if (!paused) {
        let hasDirty = false;
        for (const v of visuals.values()) {
          if (v.dirtyPolyline) {
            hasDirty = true;
            break;
          }
        }
        if (hasDirty) schedulePolylineFlush();
        if (following || interacting) startTickLoop();
      }
    },
    isFollowing: () => following,
    isPainted: () => painted,
    flightCount: () => flights.size,
    pointCount: (id: string) => flights.get(id)?.points.length ?? 0,
    getQualityLevel: () => globe.getQualityLevel(),
    resize: () => globe.resize(),
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      stopTickLoop();
      if (polylineTimer != null) {
        clearTimeout(polylineTimer);
        polylineTimer = null;
      }
      offGeoidReady();
      detachGestures();
      removeTerrainClamp();
      globe.destroy();
    },
  };
}
