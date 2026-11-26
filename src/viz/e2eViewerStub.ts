import type { FlightTag, Sample, TrackPoint } from "../track/types";
import type { FlightViewer, PathMode } from "./viewer";
import type { FollowMode } from "./follow";

/** Minimal FlightViewer for Playwright (skips Cesium WebGL boot). */
export function createE2eFlightViewer(): FlightViewer {
  return {
    setFlight: async () => {},
    setColorMode: async () => {},
    setTrim: async () => {},
    setTags: (_tags: FlightTag[]) => {},
    setSample: (_sample: Sample, _follow: boolean) => {},
    seekSample: (_sample: Sample, _follow: boolean) => {},
    setFollowMode: (_mode: FollowMode) => {},
    captureFollowFromCamera: (_sample: Sample) => {},
    unlockFollow: () => {},
    setOnTagPick: (_handler: (timeMs: number) => void) => {},
    setPathMode: (_mode: PathMode, _timeMs: number) => {},
    flyOverview: () => {},
    getPerfStats: () => ({
      commands: 0,
      triangles: 0,
      quality: 1,
      farKm: 0,
      tilesLoaded: true,
      resolutionScale: 1,
    }),
    stepQuality: (_fps: number) => {},
    appendLivePoints: async (_newPoints: TrackPoint[]) => {},
    setAccuracyDim: (_thresholdM: number) => {},
    getAircraftPosition: () => null,
    resize: () => {},
    destroy: () => {},
  };
}