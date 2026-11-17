import { describe, expect, it } from "vitest";
import {
  applyFixDedupe,
  dedupeFixes,
  parseFixEventData,
  replayFixes,
  type LiveStreamCallbacks,
  connectLiveStream,
} from "./sseClient";
import type { LiveFix } from "./types";

const fix = (t: number): LiveFix => ({
  t,
  lat: -33.4,
  lon: -70.6,
  alt: 1200,
  alt_ref: "wgs84_ellipsoid",
});

describe("sseClient parse/dedup", () => {
  it("parses fix JSON", () => {
    const data = JSON.stringify(fix(1000));
    expect(parseFixEventData(data)?.t).toBe(1000);
    expect(parseFixEventData("not json")).toBeNull();
  });

  it("dedupes fixes by timestamp", () => {
    const seen = new Set<number>();
    const batch = [fix(1), fix(2), fix(1), fix(3)];
    const fresh = dedupeFixes(batch, seen);
    expect(fresh.map((f) => f.t)).toEqual([1, 2, 3]);
    expect(applyFixDedupe(fix(2), seen)).toBe(false);
    expect(applyFixDedupe(fix(4), seen)).toBe(true);
  });

  it("replays reconnect backfill without duplicates", () => {
    const seen = new Set<number>([1, 2]);
    const incoming = [fix(2), fix(3), fix(4)];
    const received: number[] = [];
    const n = replayFixes(incoming, seen, (f) => received.push(f.t));
    expect(n).toBe(2);
    expect(received).toEqual([3, 4]);
  });
});

describe("connectLiveStream", () => {
  it("delivers fixes and dedupes on duplicate events", () => {
    const fixes: number[] = [];
    const listeners = new Map<string, Array<(ev: MessageEvent) => void>>();
    let instance = 0;

    class MockSource {
      url: string;
      onerror: (() => void) | null = null;
      constructor(url: string) {
        this.url = url;
        instance += 1;
      }
      addEventListener(type: string, fn: (ev: MessageEvent) => void): void {
        const list = listeners.get(type) ?? [];
        list.push(fn);
        listeners.set(type, list);
      }
      close(): void {}
    }

    const callbacks: LiveStreamCallbacks = {
      onFix: (f) => fixes.push(f.t),
      onEnd: () => {},
      onError: () => {},
    };

    const handle = connectLiveStream(
      "sess-1",
      callbacks,
      (url) => new MockSource(url) as unknown as EventSource,
    );

    const emit = (type: string, data: string) => {
      for (const fn of listeners.get(type) ?? []) {
        fn({ data } as MessageEvent);
      }
    };

    emit("fix", JSON.stringify(fix(10)));
    emit("fix", JSON.stringify(fix(10)));
    emit("fix", JSON.stringify(fix(11)));

    expect(fixes).toEqual([10, 11]);
    expect(instance).toBe(1);
    handle.close();
  });
});