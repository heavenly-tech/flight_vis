/**
 * Pure v1 JS bridge protocol parser, validator, flight state merger, and outbound emitter
 * for `/embed/3d?ui=mobile&v=1`. Treats all inbound payloads as untrusted input.
 */

export const BRIDGE_PROTOCOL_VERSION = 1 as const;
export const MAX_FLIGHTS = 50;
export const MAX_POINTS_PER_FLIGHT_PER_MESSAGE = 5_000;
export const MAX_STORED_POINTS_PER_FLIGHT = 10_000;
export const MAX_FLIGHT_ID_LEN = 64;
export const MAX_TAIL_LEN = 16;
export const MIN_ALT_M = -500;
export const MAX_ALT_M = 20_000;
export const PERF_EMIT_INTERVAL_MS = 10_000;

export type BridgeLang = "es" | "en";
export type BridgeUnits = "ft" | "m";
export type BridgeTailSource = "club" | "pilot";
export type BridgeLayer = "own" | "club" | "public";
export type BridgePoint = readonly [tMs: number, lat: number, lon: number, altMslM: number | null];

export type BridgeFlightInput = {
  id: string;
  tail: string;
  tailSource: BridgeTailSource;
  layer: BridgeLayer;
  points: BridgePoint[];
};

export type StoredFlight = {
  id: string;
  tail: string;
  tailSource: BridgeTailSource;
  layer: BridgeLayer;
  points: BridgePoint[];
};

export type InitMessage = {
  type: "init";
  protocol: 1;
  lang: BridgeLang;
  ownFlightId?: string;
  units?: BridgeUnits;
};

export type FlightsMessage = {
  type: "flights";
  mode: "full" | "delta";
  flights: BridgeFlightInput[];
};

export type FollowMessage = {
  type: "follow";
  on: boolean;
  flightId?: string;
};

export type RecenterMessage = {
  type: "recenter";
};

export type OverviewMessage = {
  type: "overview";
};

export type PauseMessage = {
  type: "pause";
};

export type ResumeMessage = {
  type: "resume";
};

export type BridgeInboundMessage =
  | InitMessage
  | FlightsMessage
  | FollowMessage
  | RecenterMessage
  | OverviewMessage
  | PauseMessage
  | ResumeMessage;

export type BridgeErrorCode = "webgl_unavailable" | "bad_message" | "unsupported_protocol";

export type BridgeOutboundEvent =
  | { type: "ready"; protocol: 1; version: string }
  | { type: "followChanged"; on: false; reason: "gesture" }
  | { type: "error"; code: BridgeErrorCode }
  | { type: "perf"; quality: number };

export type ParseBridgeResult =
  | { ok: true; message: BridgeInboundMessage }
  | { ok: false; code: "bad_message" | "unsupported_protocol" };

const CONTROL_CHARS_RE = /[\x00-\x1f\x7f]/;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

function ownProp(obj: Record<string, unknown>, key: string): unknown {
  return Object.prototype.hasOwnProperty.call(obj, key) ? obj[key] : undefined;
}

function isValidBoundedString(value: unknown, maxLen: number): value is string {
  if (typeof value !== "string") return false;
  if (value.length === 0 || value.length > maxLen) return false;
  if (CONTROL_CHARS_RE.test(value)) return false;
  return value.trim().length > 0;
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function parseFlightInput(raw: unknown): BridgeFlightInput | null {
  if (!isPlainObject(raw)) return null;
  const id = ownProp(raw, "id");
  const tail = ownProp(raw, "tail");
  const tailSource = ownProp(raw, "tailSource");
  const layer = ownProp(raw, "layer");
  const rawPoints = ownProp(raw, "points");

  if (!isValidBoundedString(id, MAX_FLIGHT_ID_LEN)) return null;
  if (!isValidBoundedString(tail, MAX_TAIL_LEN)) return null;
  if (tailSource !== "club" && tailSource !== "pilot") return null;
  if (layer !== "own" && layer !== "club" && layer !== "public") return null;
  if (!Array.isArray(rawPoints) || rawPoints.length > MAX_POINTS_PER_FLIGHT_PER_MESSAGE) {
    return null;
  }

  const points: BridgePoint[] = [];
  for (let i = 0; i < rawPoints.length; i += 1) {
    const pt = rawPoints[i];
    if (!Array.isArray(pt) || pt.length !== 4) return null;
    const [tMs, lat, lon, alt] = pt;
    if (!isFiniteNumber(tMs)) return null;
    if (!isFiniteNumber(lat) || lat < -90 || lat > 90) return null;
    if (!isFiniteNumber(lon) || lon < -180 || lon > 180) return null;

    let cleanAlt: number | null;
    if (alt === null) {
      cleanAlt = null;
    } else if (isFiniteNumber(alt)) {
      // Drop points whose altitude falls outside [-500, 20000] m without rejecting the batch.
      if (alt < MIN_ALT_M || alt > MAX_ALT_M) continue;
      cleanAlt = alt;
    } else {
      return null;
    }
    points.push([tMs, lat, lon, cleanAlt]);
  }

  return {
    id,
    tail,
    tailSource,
    layer,
    points,
  };
}

export function parseBridgeMessage(raw: unknown): ParseBridgeResult {
  let obj: unknown = raw;
  if (typeof raw === "string") {
    try {
      obj = JSON.parse(raw);
    } catch {
      return { ok: false, code: "bad_message" };
    }
  }
  if (!isPlainObject(obj)) {
    return { ok: false, code: "bad_message" };
  }

  const type = ownProp(obj, "type");
  if (typeof type !== "string") {
    return { ok: false, code: "bad_message" };
  }

  switch (type) {
    case "init": {
      const protocol = ownProp(obj, "protocol");
      if (!isFiniteNumber(protocol)) {
        return { ok: false, code: "bad_message" };
      }
      if (protocol !== BRIDGE_PROTOCOL_VERSION) {
        return { ok: false, code: "unsupported_protocol" };
      }
      const lang = ownProp(obj, "lang");
      if (lang !== "es" && lang !== "en") {
        return { ok: false, code: "bad_message" };
      }
      const rawOwnFlightId = ownProp(obj, "ownFlightId");
      let ownFlightId: string | undefined;
      if (rawOwnFlightId !== undefined && rawOwnFlightId !== null) {
        if (!isValidBoundedString(rawOwnFlightId, MAX_FLIGHT_ID_LEN)) {
          return { ok: false, code: "bad_message" };
        }
        ownFlightId = rawOwnFlightId;
      }
      const rawUnits = ownProp(obj, "units");
      let units: BridgeUnits | undefined;
      if (rawUnits !== undefined && rawUnits !== null) {
        if (rawUnits !== "ft" && rawUnits !== "m") {
          return { ok: false, code: "bad_message" };
        }
        units = rawUnits;
      }
      return {
        ok: true,
        message: {
          type: "init",
          protocol: 1,
          lang,
          ...(ownFlightId !== undefined ? { ownFlightId } : {}),
          ...(units !== undefined ? { units } : {}),
        },
      };
    }

    case "flights": {
      const mode = ownProp(obj, "mode");
      if (mode !== "full" && mode !== "delta") {
        return { ok: false, code: "bad_message" };
      }
      const rawFlights = ownProp(obj, "flights");
      if (!Array.isArray(rawFlights) || rawFlights.length > MAX_FLIGHTS) {
        return { ok: false, code: "bad_message" };
      }
      const flights: BridgeFlightInput[] = [];
      for (let i = 0; i < rawFlights.length; i += 1) {
        const parsed = parseFlightInput(rawFlights[i]);
        if (!parsed) {
          return { ok: false, code: "bad_message" };
        }
        flights.push(parsed);
      }
      return {
        ok: true,
        message: {
          type: "flights",
          mode,
          flights,
        },
      };
    }

    case "follow": {
      const on = ownProp(obj, "on");
      if (typeof on !== "boolean") {
        return { ok: false, code: "bad_message" };
      }
      const rawFlightId = ownProp(obj, "flightId");
      let flightId: string | undefined;
      if (rawFlightId !== undefined && rawFlightId !== null) {
        if (!isValidBoundedString(rawFlightId, MAX_FLIGHT_ID_LEN)) {
          return { ok: false, code: "bad_message" };
        }
        flightId = rawFlightId;
      }
      return {
        ok: true,
        message: {
          type: "follow",
          on,
          ...(flightId !== undefined ? { flightId } : {}),
        },
      };
    }

    case "recenter":
      return { ok: true, message: { type: "recenter" } };
    case "overview":
      return { ok: true, message: { type: "overview" } };
    case "pause":
      return { ok: true, message: { type: "pause" } };
    case "resume":
      return { ok: true, message: { type: "resume" } };
    default:
      return { ok: false, code: "bad_message" };
  }
}

function normalizePoints(points: readonly BridgePoint[]): BridgePoint[] {
  if (points.length <= 1) return points.slice();
  let sorted = true;
  for (let i = 1; i < points.length; i += 1) {
    if (points[i][0] <= points[i - 1][0]) {
      sorted = false;
      break;
    }
  }
  const ordered = sorted ? points.slice() : [...points].sort((a, b) => a[0] - b[0]);
  const deduped: BridgePoint[] = [];
  for (const pt of ordered) {
    const last = deduped[deduped.length - 1];
    if (last && last[0] === pt[0]) {
      deduped[deduped.length - 1] = pt;
    } else {
      deduped.push(pt);
    }
  }
  if (deduped.length > MAX_STORED_POINTS_PER_FLIGHT) {
    return deduped.slice(deduped.length - MAX_STORED_POINTS_PER_FLIGHT);
  }
  return deduped;
}

function mergePoints(
  existing: readonly BridgePoint[],
  incoming: readonly BridgePoint[],
): { points: BridgePoint[]; appendedStrictly: boolean; appendedCount: number } {
  if (incoming.length === 0) {
    return { points: existing.slice(), appendedStrictly: true, appendedCount: 0 };
  }
  const cleanIncoming = normalizePoints(incoming);
  if (existing.length === 0) {
    return {
      points: cleanIncoming,
      appendedStrictly: true,
      appendedCount: cleanIncoming.length,
    };
  }
  const lastExistingT = existing[existing.length - 1][0];
  if (cleanIncoming[0][0] > lastExistingT) {
    const combined = [...existing, ...cleanIncoming];
    if (combined.length > MAX_STORED_POINTS_PER_FLIGHT) {
      return {
        points: combined.slice(combined.length - MAX_STORED_POINTS_PER_FLIGHT),
        appendedStrictly: false,
        appendedCount: cleanIncoming.length,
      };
    }
    return {
      points: combined,
      appendedStrictly: true,
      appendedCount: cleanIncoming.length,
    };
  }
  const merged = normalizePoints([...existing, ...cleanIncoming]);
  return {
    points: merged,
    appendedStrictly: false,
    appendedCount: Math.max(0, merged.length - existing.length),
  };
}

export type FlightsUpdateDiff = {
  flights: Map<string, StoredFlight>;
  added: string[];
  updated: string[];
  appendedStrictly: Set<string>;
  removed: string[];
};

export function applyFlightsUpdate(
  current: ReadonlyMap<string, StoredFlight>,
  msg: FlightsMessage,
): FlightsUpdateDiff {
  const added: string[] = [];
  const updated: string[] = [];
  const appendedStrictly = new Set<string>();
  const removed: string[] = [];

  if (msg.mode === "full") {
    const next = new Map<string, StoredFlight>();
    for (const incoming of msg.flights) {
      if (next.size >= MAX_FLIGHTS && !next.has(incoming.id)) continue;
      const normalized: StoredFlight = {
        id: incoming.id,
        tail: incoming.tail,
        tailSource: incoming.tailSource,
        layer: incoming.layer,
        points: normalizePoints(incoming.points),
      };
      const existed = current.has(incoming.id);
      next.set(incoming.id, normalized);
      if (existed) {
        if (!updated.includes(incoming.id)) updated.push(incoming.id);
      } else if (!added.includes(incoming.id)) {
        added.push(incoming.id);
      }
    }
    for (const oldId of current.keys()) {
      if (!next.has(oldId)) removed.push(oldId);
    }
    return { flights: next, added, updated, appendedStrictly, removed };
  }

  // Delta mode: merge into existing flights and respect the 50-flight cap.
  const next = new Map<string, StoredFlight>();
  for (const [id, flight] of current.entries()) {
    next.set(id, flight);
  }
  for (const incoming of msg.flights) {
    const prev = next.get(incoming.id);
    if (prev) {
      const merged = mergePoints(prev.points, incoming.points);
      next.set(incoming.id, {
        id: incoming.id,
        tail: incoming.tail,
        tailSource: incoming.tailSource,
        layer: incoming.layer,
        points: merged.points,
      });
      if (!updated.includes(incoming.id)) updated.push(incoming.id);
      if (merged.appendedStrictly) {
        appendedStrictly.add(incoming.id);
      } else {
        appendedStrictly.delete(incoming.id);
      }
    } else if (next.size < MAX_FLIGHTS) {
      next.set(incoming.id, {
        id: incoming.id,
        tail: incoming.tail,
        tailSource: incoming.tailSource,
        layer: incoming.layer,
        points: normalizePoints(incoming.points),
      });
      if (!added.includes(incoming.id)) added.push(incoming.id);
    }
  }
  return { flights: next, added, updated, appendedStrictly, removed };
}

export type BridgeHostWindow = {
  FvBridge?: {
    postMessage?: (message: string) => void;
  };
  dispatchEvent?: (event: Event) => boolean;
};

export function emitBridgeEvent(
  event: BridgeOutboundEvent,
  host: BridgeHostWindow | undefined = typeof window !== "undefined"
    ? (window as unknown as BridgeHostWindow)
    : undefined,
): void {
  if (!host) return;
  const serialized = JSON.stringify(event);
  if (host.FvBridge && typeof host.FvBridge.postMessage === "function") {
    try {
      host.FvBridge.postMessage(serialized);
    } catch {
      /* isolate host channel exceptions */
    }
  }
  if (typeof host.dispatchEvent === "function" && typeof CustomEvent !== "undefined") {
    host.dispatchEvent(new CustomEvent("fvbridge", { detail: event }));
  }
}

export type PerfEmitter = {
  maybeEmit: (quality: number, paused?: boolean) => boolean;
  reset: () => void;
};

export function createPerfEmitter(
  emit: (event: { type: "perf"; quality: number }) => void,
  intervalMs = PERF_EMIT_INTERVAL_MS,
  nowFn: () => number = () => Date.now(),
): PerfEmitter {
  let lastEmitAt = Number.NEGATIVE_INFINITY;
  return {
    maybeEmit: (quality: number, paused = false) => {
      if (paused || !Number.isFinite(quality)) return false;
      const now = nowFn();
      if (now - lastEmitAt < intervalMs) return false;
      lastEmitAt = now;
      emit({ type: "perf", quality });
      return true;
    },
    reset: () => {
      lastEmitAt = Number.NEGATIVE_INFINITY;
    },
  };
}
