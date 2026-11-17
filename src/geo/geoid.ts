import grid from "./egm96-grid.json";

type Egm96Grid = {
  minLat: number;
  maxLat: number;
  minLon: number;
  maxLon: number;
  stepDeg: number;
  undulationM: number[][];
};

const G = grid as Egm96Grid;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Geoid undulation N (meters): ellipsoidal height = MSL + N. */
export function geoidUndulationM(lat: number, lon: number): number {
  const latClamped = clamp(lat, G.minLat, G.maxLat - 1e-9);
  const lonClamped = clamp(lon, G.minLon, G.maxLon - 1e-9);

  const latIdx = (G.maxLat - latClamped) / G.stepDeg;
  const lonIdx = (lonClamped - G.minLon) / G.stepDeg;

  const lat0 = Math.floor(latIdx);
  const lon0 = Math.floor(lonIdx);
  const lat1 = Math.min(lat0 + 1, G.undulationM.length - 1);
  const lon1 = Math.min(lon0 + 1, G.undulationM[0].length - 1);

  const ty = latIdx - lat0;
  const tx = lonIdx - lon0;

  const n00 = G.undulationM[lat0][lon0];
  const n01 = G.undulationM[lat0][lon1];
  const n10 = G.undulationM[lat1][lon0];
  const n11 = G.undulationM[lat1][lon1];

  const n0 = n00 * (1 - tx) + n01 * tx;
  const n1 = n10 * (1 - tx) + n11 * tx;
  return n0 * (1 - ty) + n1 * ty;
}

/** Convert WGS84 ellipsoidal height to MSL using a coarse EGM96 grid. */
export function ellipsoidToMsl(lat: number, lon: number, hEllipsoid: number): number {
  return hEllipsoid - geoidUndulationM(lat, lon);
}