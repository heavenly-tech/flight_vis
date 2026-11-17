/**
 * EGM96 geoid undulation via NGA EGM96 sample grid (bilinear interpolation).
 * Data shipped in `egm96-universal` (MIT), tested against the NGA Fortran reference.
 * @see https://earth-info.nga.mil/GandG/wgs84/gravitymod/egm96/binary/binarygeoid.html
 */
import { ellipsoidToEgm96, meanSeaLevel } from "egm96-universal";

/** Geoid undulation N (m): ellipsoidal height = MSL + N. */
export function geoidUndulationM(lat: number, lon: number): number {
  return meanSeaLevel(lat, lon);
}

/** Convert WGS84 ellipsoidal height to MSL (EGM96). */
export function ellipsoidToMsl(lat: number, lon: number, hEllipsoid: number): number {
  return ellipsoidToEgm96(lat, lon, hEllipsoid);
}