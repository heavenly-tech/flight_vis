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
  SceneMode,
  Transforms,
  UrlTemplateImageryProvider,
  VerticalOrigin,
  Viewer,
  createWorldTerrainAsync,
} from "cesium";
import type { ColorMode, Flight, FlightTag, Sample, TrimRange } from "../track/types";
import { fullRange, pointsInTrim } from "../track/trim";
import { colorForUnit, normalizeValue, valueForMode } from "./colors";

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
  flyOverview: () => void;
  destroy: () => void;
};

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
        maximumLevel: 19,
      }),
    ),
    sceneMode: SceneMode.SCENE3D,
    requestRenderMode: false,
  });

  viewer.scene.globe.enableLighting = true;
  viewer.scene.globe.depthTestAgainstTerrain = false;
  viewer.scene.fog.enabled = true;
  if (viewer.scene.skyAtmosphere) viewer.scene.skyAtmosphere.show = true;
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
  let trackPrimitive: Primitive | undefined;
  let craft: Entity | undefined;
  let tagEntities: Entity[] = [];
  let trackPositions: Cartesian3[] = [];

  function unlockCamera(): void {
    viewer.camera.lookAtTransform(Matrix4.IDENTITY);
  }

  function clearTags(): void {
    for (const entity of tagEntities) viewer.entities.remove(entity);
    tagEntities = [];
  }

  function drawTags(): void {
    clearTags();
    if (!flight) return;
    for (const tag of tags) {
      const point = flight.points.reduce((best, p) =>
        Math.abs(p.time - tag.timeMs) < Math.abs(best.time - tag.timeMs) ? p : best,
      );
      tagEntities.push(
        viewer.entities.add({
          position: Cartesian3.fromDegrees(point.lon, point.lat, point.alt + 40),
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

  async function rebuildTrack(next: Flight, mode: ColorMode, nextTrim: TrimRange): Promise<void> {
    flight = next;
    colorMode = mode;
    trim = nextTrim;
    if (trackPrimitive) {
      viewer.scene.primitives.remove(trackPrimitive);
      trackPrimitive = undefined;
    }
    if (craft) {
      viewer.entities.remove(craft);
      craft = undefined;
    }

    const visible = pointsInTrim(next, nextTrim);
    const source = visible.length >= 2 ? visible : next.points;
    const positions = source.map((p) => Cartesian3.fromDegrees(p.lon, p.lat, p.alt));
    trackPositions = positions;
    const colors = source.map((p) => colorForUnit(normalizeValue(next, mode, valueForMode(p, mode))));

    trackPrimitive = viewer.scene.primitives.add(
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
    );

    const start = JulianDate.fromDate(new Date(source[0].time));
    const stop = JulianDate.fromDate(new Date(source[source.length - 1].time));
    viewer.clock.startTime = start;
    viewer.clock.stopTime = stop;
    viewer.clock.currentTime = JulianDate.clone(start);
    viewer.clock.clockRange = ClockRange.CLAMPED;
    viewer.clock.shouldAnimate = false;

    craft = viewer.entities.add({
      position: positions[0],
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

    if (!followCam) {
      unlockCamera();
      return;
    }

    const hpr = new HeadingPitchRoll(sample.point.headingRad, 0, 0);
    craft.orientation = new ConstantProperty(Transforms.headingPitchRollQuaternion(position, hpr));
    const range = Math.max(
      1800,
      Math.min(14000, 2400 + sample.point.speedMps * 28 + sample.point.alt * 1.1),
    );
    viewer.camera.lookAt(
      position,
      new HeadingPitchRange(
        sample.point.headingRad - CesiumMath.PI_OVER_TWO,
        CesiumMath.toRadians(-32),
        range,
      ),
    );
  }

  function flyOverview(): void {
    if (!flight || trackPositions.length === 0) return;
    unlockCamera();
    const sphere = BoundingSphere.fromPoints(trackPositions);
    viewer.camera.flyToBoundingSphere(sphere, {
      offset: new HeadingPitchRange(0, CesiumMath.toRadians(-48), Math.max(sphere.radius * 2.1, 8000)),
      duration: 1.2,
    });
  }

  return {
    setFlight: async (next, mode, nextTrim) => {
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
      drawTags();
    },
    setSample,
    flyOverview,
    destroy: () => viewer.destroy(),
  };
}
