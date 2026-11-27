import { describe, expect, it, vi } from "vitest";
import {
  BRIDGE_PROTOCOL_VERSION,
  MAX_ALT_M,
  MAX_FLIGHTS,
  MAX_FLIGHT_ID_LEN,
  MAX_POINTS_PER_FLIGHT_PER_MESSAGE,
  MAX_STORED_POINTS_PER_FLIGHT,
  MAX_TAIL_LEN,
  MIN_ALT_M,
  applyFlightsUpdate,
  createPerfEmitter,
  emitBridgeEvent,
  parseBridgeMessage,
  type BridgeFlightInput,
  type BridgeHostWindow,
  type BridgeOutboundEvent,
  type FlightsMessage,
  type StoredFlight,
} from "./bridge";

function sampleFlight(overrides: Partial<BridgeFlightInput> = {}): BridgeFlightInput {
  return {
    id: "flight-1",
    tail: "CC-ABC",
    tailSource: "club",
    layer: "own",
    points: [
      [1_700_000_000_000, -33.45, -70.66, 1200],
      [1_700_000_005_000, -33.44, -70.65, 1250],
    ],
    ...overrides,
  };
}

describe("parseBridgeMessage", () => {
  it("accepts valid init from object and JSON string", () => {
    const objMsg = {
      type: "init",
      protocol: 1,
      lang: "es",
      ownFlightId: "f-own",
      units: "ft",
    };
    const resObj = parseBridgeMessage(objMsg);
    expect(resObj).toEqual({
      ok: true,
      message: {
        type: "init",
        protocol: 1,
        lang: "es",
        ownFlightId: "f-own",
        units: "ft",
      },
    });

    const resStr = parseBridgeMessage(JSON.stringify({ type: "init", protocol: 1, lang: "en" }));
    expect(resStr).toEqual({
      ok: true,
      message: { type: "init", protocol: 1, lang: "en" },
    });
  });

  it("rejects wrong protocol with unsupported_protocol and non-finite protocol with bad_message", () => {
    expect(parseBridgeMessage({ type: "init", protocol: 2, lang: "es" })).toEqual({
      ok: false,
      code: "unsupported_protocol",
    });
    expect(parseBridgeMessage({ type: "init", protocol: 0, lang: "en" })).toEqual({
      ok: false,
      code: "unsupported_protocol",
    });
    expect(parseBridgeMessage({ type: "init", protocol: Number.NaN, lang: "es" })).toEqual({
      ok: false,
      code: "bad_message",
    });
    expect(parseBridgeMessage({ type: "init", protocol: "1", lang: "es" })).toEqual({
      ok: false,
      code: "bad_message",
    });
  });

  it("rejects invalid init fields (lang, ownFlightId length/control chars, units)", () => {
    expect(parseBridgeMessage({ type: "init", protocol: 1, lang: "fr" })).toEqual({
      ok: false,
      code: "bad_message",
    });
    expect(
      parseBridgeMessage({
        type: "init",
        protocol: 1,
        lang: "es",
        ownFlightId: "x".repeat(MAX_FLIGHT_ID_LEN + 1),
      }),
    ).toEqual({ ok: false, code: "bad_message" });
    expect(
      parseBridgeMessage({
        type: "init",
        protocol: 1,
        lang: "es",
        ownFlightId: "bad\nid",
      }),
    ).toEqual({ ok: false, code: "bad_message" });
    expect(
      parseBridgeMessage({ type: "init", protocol: 1, lang: "es", units: "km" }),
    ).toEqual({ ok: false, code: "bad_message" });
  });

  it("rejects malformed JSON, non-objects, arrays, and unknown message types", () => {
    for (const bad of [
      "{not json",
      null,
      undefined,
      42,
      true,
      [],
      {},
      { type: "unknown" },
      { type: "eval", code: "alert(1)" },
    ]) {
      expect(parseBridgeMessage(bad)).toEqual({ ok: false, code: "bad_message" });
    }
  });

  it("validates follow, recenter, overview, pause, and resume messages", () => {
    expect(parseBridgeMessage({ type: "follow", on: true, flightId: "f-1" })).toEqual({
      ok: true,
      message: { type: "follow", on: true, flightId: "f-1" },
    });
    expect(parseBridgeMessage({ type: "follow", on: false })).toEqual({
      ok: true,
      message: { type: "follow", on: false },
    });
    expect(parseBridgeMessage({ type: "follow", on: "yes" })).toEqual({
      ok: false,
      code: "bad_message",
    });
    expect(
      parseBridgeMessage({ type: "follow", on: true, flightId: "a".repeat(MAX_FLIGHT_ID_LEN + 1) }),
    ).toEqual({ ok: false, code: "bad_message" });

    for (const simple of ["recenter", "overview", "pause", "resume"] as const) {
      expect(parseBridgeMessage({ type: simple })).toEqual({
        ok: true,
        message: { type: simple },
      });
    }
  });

  it("enforces flight count cap (50), point count cap (5000), id (64), and tail (16)", () => {
    const fifty = Array.from({ length: MAX_FLIGHTS }, (_, i) =>
      sampleFlight({ id: `f-${i}`, tail: `CC-${i}` }),
    );
    expect(parseBridgeMessage({ type: "flights", mode: "full", flights: fifty }).ok).toBe(true);

    const fiftyOne = [...fifty, sampleFlight({ id: "f-50", tail: "CC-50" })];
    expect(parseBridgeMessage({ type: "flights", mode: "full", flights: fiftyOne })).toEqual({
      ok: false,
      code: "bad_message",
    });

    const maxPoints = Array.from({ length: MAX_POINTS_PER_FLIGHT_PER_MESSAGE }, (_, i) => [
      1_700_000_000_000 + i * 1000,
      -33.4,
      -70.6,
      1000,
    ]);
    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "delta",
        flights: [sampleFlight({ points: maxPoints as BridgeFlightInput["points"] })],
      }).ok,
    ).toBe(true);

    const tooManyPoints = [...maxPoints, [1_700_009_999_000, -33.4, -70.6, 1000]];
    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "delta",
        flights: [sampleFlight({ points: tooManyPoints as BridgeFlightInput["points"] })],
      }),
    ).toEqual({ ok: false, code: "bad_message" });

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "full",
        flights: [sampleFlight({ id: "a".repeat(MAX_FLIGHT_ID_LEN + 1) })],
      }),
    ).toEqual({ ok: false, code: "bad_message" });

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "full",
        flights: [sampleFlight({ tail: "T".repeat(MAX_TAIL_LEN + 1) })],
      }),
    ).toEqual({ ok: false, code: "bad_message" });

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "full",
        flights: [sampleFlight({ tail: "" })],
      }),
    ).toEqual({ ok: false, code: "bad_message" });
  });

  it("rejects non-finite numbers and out-of-range lat/lon, and rejects invalid enums", () => {
    const badPoints: unknown[] = [
      [Number.NaN, -33.4, -70.6, 1000],
      [Number.POSITIVE_INFINITY, -33.4, -70.6, 1000],
      [1000, -90.1, -70.6, 1000],
      [1000, 90.1, -70.6, 1000],
      [1000, -33.4, -180.1, 1000],
      [1000, -33.4, 180.1, 1000],
      [1000, -33.4, -70.6, Number.NaN],
      [1000, -33.4, -70.6, Number.NEGATIVE_INFINITY],
      [1000, -33.4, -70.6, "1000"],
      [1000, -33.4, -70.6],
    ];
    for (const pt of badPoints) {
      expect(
        parseBridgeMessage({
          type: "flights",
          mode: "full",
          flights: [{ ...sampleFlight(), points: [pt] }],
        }),
      ).toEqual({ ok: false, code: "bad_message" });
    }

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "replace",
        flights: [sampleFlight()],
      }),
    ).toEqual({ ok: false, code: "bad_message" });

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "full",
        flights: [{ ...sampleFlight(), tailSource: "other" }],
      }),
    ).toEqual({ ok: false, code: "bad_message" });

    expect(
      parseBridgeMessage({
        type: "flights",
        mode: "full",
        flights: [{ ...sampleFlight(), layer: "private" }],
      }),
    ).toEqual({ ok: false, code: "bad_message" });
  });

  it("drops points with altitude outside [-500, 20000] m while preserving null and boundary altitudes", () => {
    const res = parseBridgeMessage({
      type: "flights",
      mode: "full",
      flights: [
        sampleFlight({
          points: [
            [1000, -33.4, -70.6, MIN_ALT_M - 0.1], // drop
            [2000, -33.4, -70.6, MIN_ALT_M], // keep
            [3000, -33.4, -70.6, null], // keep
            [4000, -33.4, -70.6, MAX_ALT_M], // keep
            [5000, -33.4, -70.6, MAX_ALT_M + 0.1], // drop
          ],
        }),
      ],
    });
    expect(res.ok).toBe(true);
    if (!res.ok || res.message.type !== "flights") throw new Error("expected flights");
    expect(res.message.flights[0].points).toEqual([
      [2000, -33.4, -70.6, -500],
      [3000, -33.4, -70.6, null],
      [4000, -33.4, -70.6, 20000],
    ]);
  });
});

describe("applyFlightsUpdate", () => {
  it("replaces and removes absent flights in full mode", () => {
    const initial = new Map<string, StoredFlight>();
    const full1: FlightsMessage = {
      type: "flights",
      mode: "full",
      flights: [
        sampleFlight({ id: "f1", tail: "CC-ONE" }),
        sampleFlight({ id: "f2", tail: "CC-TWO", layer: "club" }),
        sampleFlight({ id: "f3", tail: "CC-PIL", tailSource: "pilot", layer: "public" }),
      ],
    };
    const d1 = applyFlightsUpdate(initial, full1);
    expect(d1.flights.size).toBe(3);
    expect(d1.added).toEqual(["f1", "f2", "f3"]);
    expect(d1.removed).toEqual([]);

    const full2: FlightsMessage = {
      type: "flights",
      mode: "full",
      flights: [sampleFlight({ id: "f1", tail: "CC-ONE" })],
    };
    const d2 = applyFlightsUpdate(d1.flights, full2);
    expect(d2.flights.size).toBe(1);
    expect(d2.removed.sort()).toEqual(["f2", "f3"]);
    expect(d2.updated).toEqual(["f1"]);
  });

  it("merges delta points with strict append fast-path, out-of-order sort, dedup, and 50-flight cap", () => {
    const base = applyFlightsUpdate(new Map(), {
      type: "flights",
      mode: "full",
      flights: [
        sampleFlight({
          id: "f1",
          points: [
            [1000, -33.45, -70.66, 1000],
            [2000, -33.44, -70.65, 1100],
          ],
        }),
      ],
    }).flights;

    // Monotonic append
    const d1 = applyFlightsUpdate(base, {
      type: "flights",
      mode: "delta",
      flights: [
        sampleFlight({
          id: "f1",
          points: [
            [3000, -33.43, -70.64, 1200],
            [4000, -33.42, -70.63, 1300],
          ],
        }),
      ],
    });
    expect(d1.appendedStrictly.has("f1")).toBe(true);
    expect(d1.flights.get("f1")?.points.map((p) => p[0])).toEqual([1000, 2000, 3000, 4000]);

    // Out-of-order + duplicate timestamp replaces same-t point and sorts
    const d2 = applyFlightsUpdate(d1.flights, {
      type: "flights",
      mode: "delta",
      flights: [
        sampleFlight({
          id: "f1",
          points: [
            [2500, -33.435, -70.645, 1150],
            [3000, -33.43, -70.64, 1220],
          ],
        }),
      ],
    });
    expect(d2.appendedStrictly.has("f1")).toBe(false);
    expect(d2.flights.get("f1")?.points).toEqual([
      [1000, -33.45, -70.66, 1000],
      [2000, -33.44, -70.65, 1100],
      [2500, -33.435, -70.645, 1150],
      [3000, -33.43, -70.64, 1220],
      [4000, -33.42, -70.63, 1300],
    ]);

    // Delta never exceeds MAX_FLIGHTS (50)
    const full50 = applyFlightsUpdate(new Map(), {
      type: "flights",
      mode: "full",
      flights: Array.from({ length: MAX_FLIGHTS }, (_, i) =>
        sampleFlight({ id: `f-${i}`, tail: `CC-${i}` }),
      ),
    }).flights;
    const dCap = applyFlightsUpdate(full50, {
      type: "flights",
      mode: "delta",
      flights: [sampleFlight({ id: "f-overflow", tail: "CC-OVR" })],
    });
    expect(dCap.flights.size).toBe(MAX_FLIGHTS);
    expect(dCap.flights.has("f-overflow")).toBe(false);
  });

  it("caps stored points per flight at MAX_STORED_POINTS_PER_FLIGHT keeping newest", () => {
    let state = new Map<string, StoredFlight>();
    for (let batch = 0; batch < 3; batch += 1) {
      const pts = Array.from({ length: 4_000 }, (_, i) => {
        const idx = batch * 4_000 + i;
        return [idx * 1000, -33.4, -70.6, 1000] as const;
      });
      state = applyFlightsUpdate(state, {
        type: "flights",
        mode: batch === 0 ? "full" : "delta",
        flights: [sampleFlight({ id: "long", points: pts })],
      }).flights;
    }
    const stored = state.get("long")!;
    expect(stored.points.length).toBe(MAX_STORED_POINTS_PER_FLIGHT);
    expect(stored.points[0][0]).toBe(2_000 * 1000);
    expect(stored.points[stored.points.length - 1][0]).toBe(11_999 * 1000);
  });
});

describe("outbound bridge events and perf emitter", () => {
  it("posts JSON to FvBridge.postMessage and dispatches fvbridge CustomEvent", () => {
    const posted: string[] = [];
    const dispatched: BridgeOutboundEvent[] = [];
    const host: BridgeHostWindow = {
      FvBridge: { postMessage: (s) => posted.push(s) },
      dispatchEvent: (ev) => {
        dispatched.push((ev as CustomEvent<BridgeOutboundEvent>).detail);
        return true;
      },
    };

    const events: BridgeOutboundEvent[] = [
      { type: "ready", protocol: BRIDGE_PROTOCOL_VERSION, version: "0.1.0" },
      { type: "followChanged", on: false, reason: "gesture" },
      { type: "error", code: "bad_message" },
      { type: "perf", quality: 1 },
    ];
    for (const ev of events) {
      emitBridgeEvent(ev, host);
    }

    expect(dispatched).toEqual(events);
    expect(posted).toEqual(events.map((e) => JSON.stringify(e)));
  });

  it("isolates FvBridge.postMessage exceptions and still dispatches CustomEvent", () => {
    const dispatched: BridgeOutboundEvent[] = [];
    const host: BridgeHostWindow = {
      FvBridge: {
        postMessage: () => {
          throw new Error("channel closed");
        },
      },
      dispatchEvent: (ev) => {
        dispatched.push((ev as CustomEvent<BridgeOutboundEvent>).detail);
        return true;
      },
    };
    const ev: BridgeOutboundEvent = { type: "error", code: "webgl_unavailable" };
    emitBridgeEvent(ev, host);
    expect(dispatched).toEqual([ev]);
  });

  it("rate-limits perf events to at most once every 10s and suppresses while paused", () => {
    let now = 100_000;
    const emit = vi.fn();
    const perf = createPerfEmitter(emit, 10_000, () => now);

    expect(perf.maybeEmit(0, true)).toBe(false);
    expect(emit).not.toHaveBeenCalled();

    expect(perf.maybeEmit(0, false)).toBe(true);
    expect(emit).toHaveBeenCalledTimes(1);
    expect(emit).toHaveBeenLastCalledWith({ type: "perf", quality: 0 });

    now += 9_999;
    expect(perf.maybeEmit(1, false)).toBe(false);
    expect(emit).toHaveBeenCalledTimes(1);

    now += 1;
    expect(perf.maybeEmit(1, false)).toBe(true);
    expect(emit).toHaveBeenCalledTimes(2);
    expect(emit).toHaveBeenLastCalledWith({ type: "perf", quality: 1 });
  });
});
