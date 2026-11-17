/** Fix timestamp in Unix milliseconds (UTC). */
export type FixTime = number;

export const ALT_REF_WGS84_ELLIPSOID = "wgs84_ellipsoid";

export type LiveFix = {
  t: FixTime;
  lat: number;
  lon: number;
  alt?: number | null;
  alt_ref?: string | null;
  speed?: number | null;
  course?: number | null;
  accuracy?: number | null;
};

export type SessionPhase = "active" | "ended";

export type LiveSessionSummary = {
  id: string;
  device: string;
  phase: SessionPhase;
  started_at: FixTime;
  ended_at?: FixTime | null;
  last_fix_at?: FixTime | null;
  fix_count: number;
};