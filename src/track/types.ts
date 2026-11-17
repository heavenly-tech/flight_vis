export type TrackPoint = {
  lat: number;
  lon: number;
  alt: number;
  time: number;
  speedMps: number;
  varioMps: number;
  distanceM: number;
  headingRad: number;
  /** WGS84 ellipsoid height for the globe; `alt` is MSL for HUD/labels. */
  positionAlt?: number;
  /** True when `alt` was derived from EGM96, not a geoid-pending placeholder. */
  mslFromGeoid?: boolean;
  accuracyM?: number;
};

export type ColorMode = "altitude" | "speed" | "vario";

export type FlightTag = {
  id: string;
  timeMs: number;
  label: string;
};

export type TrimRange = {
  startMs: number;
  endMs: number;
};

export type Flight = {
  name: string;
  source: "gpx" | "igc";
  fileName: string;
  points: TrackPoint[];
  durationMs: number;
  distanceM: number;
  minAlt: number;
  maxAlt: number;
  maxSpeedMps: number;
  minVario: number;
  maxVario: number;
};

export type Sample = {
  point: TrackPoint;
  index: number;
  t: number;
};
