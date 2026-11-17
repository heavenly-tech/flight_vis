/**
 * EGM96 geoid undulation (lazy-loaded 0.25° grid derived from NGA EGM96).
 * @see https://earth-info.nga.mil/GandG/wgs84/gravitymod/egm96/binary/binarygeoid.html
 */

export type GeoidMeta = {
  stepDeg: number;
  latMin: number;
  latMax: number;
  lonMin: number;
  lonMax: number;
  latCount: number;
  lonCount: number;
};

const GRID_BASE = `${import.meta.env.BASE_URL}geoid/`;

function geoidAssetUrl(name: string): string {
  const rel = `${GRID_BASE}${name}`;
  if (typeof window !== "undefined" && window.location?.origin) {
    return new URL(rel, window.location.origin).href;
  }
  return new URL(rel, "http://localhost/").href;
}

let meta: GeoidMeta | null = null;
let undulation: Float32Array | null = null;
let loadPromise: Promise<void> | null = null;
const readyWaiters: Array<() => void> = [];

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function bilinear(meta: GeoidMeta, grid: Float32Array, lat: number, lon: number): number {
  const latClamped = clamp(lat, meta.latMin, meta.latMax - 1e-9);
  let lonNorm = lon;
  while (lonNorm < meta.lonMin) lonNorm += 360;
  while (lonNorm > meta.lonMax + meta.stepDeg) lonNorm -= 360;
  const lonClamped = clamp(lonNorm, meta.lonMin, meta.lonMax);

  const latIdx = (meta.latMax - latClamped) / meta.stepDeg;
  const lonIdx = (lonClamped - meta.lonMin) / meta.stepDeg;

  const lat0 = Math.floor(latIdx);
  const lon0 = Math.floor(lonIdx);
  const lat1 = Math.min(lat0 + 1, meta.latCount - 1);
  const lon1 = Math.min(lon0 + 1, meta.lonCount - 1);

  const ty = latIdx - lat0;
  const tx = lonIdx - lon0;

  const at = (r: number, c: number) => grid[r * meta.lonCount + c]!;
  const n00 = at(lat0, lon0);
  const n01 = at(lat0, lon1);
  const n10 = at(lat1, lon0);
  const n11 = at(lat1, lon1);

  const n0 = n00 * (1 - tx) + n01 * tx;
  const n1 = n10 * (1 - tx) + n11 * tx;
  return n0 * (1 - ty) + n1 * ty;
}

/** @internal test hook — inject a grid without fetch. */
export function installGeoidGrid(nextMeta: GeoidMeta, values: Float32Array): void {
  meta = nextMeta;
  undulation = values;
  loadPromise = Promise.resolve();
  for (const wake of readyWaiters.splice(0)) wake();
}

export function isGeoidReady(): boolean {
  return undulation != null && meta != null;
}

export function whenGeoidReady(): Promise<void> {
  if (isGeoidReady()) return Promise.resolve();
  return ensureGeoidLoaded();
}

export async function ensureGeoidLoaded(): Promise<void> {
  if (isGeoidReady()) return;
  if (!loadPromise) {
    loadPromise = (async () => {
      const metaRes = await fetch(geoidAssetUrl("egm96-025deg.json"));
      if (!metaRes.ok) throw new Error("Could not load geoid metadata");
      const nextMeta = (await metaRes.json()) as GeoidMeta;
      const binRes = await fetch(geoidAssetUrl("egm96-025deg.bin"));
      if (!binRes.ok) throw new Error("Could not load geoid grid");
      const buffer = await binRes.arrayBuffer();
      installGeoidGrid(nextMeta, new Float32Array(buffer));
    })().catch((error) => {
      loadPromise = null;
      throw error;
    });
  }
  await loadPromise;
}

/** Geoid undulation N (m): ellipsoidal height = MSL + N. Requires loaded grid. */
export function geoidUndulationM(lat: number, lon: number): number {
  if (!undulation || !meta) throw new Error("Geoid grid not loaded");
  return bilinear(meta, undulation, lat, lon);
}

/** Convert WGS84 ellipsoidal height to MSL (EGM96). Requires loaded grid. */
export function ellipsoidToMsl(lat: number, lon: number, hEllipsoid: number): number {
  return hEllipsoid - geoidUndulationM(lat, lon);
}