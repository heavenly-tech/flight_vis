import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ensureGeoidLoaded,
  isGeoidReady,
  resetGeoidForTests,
  type GeoidMeta,
} from "./geoid";

const root = path.resolve(import.meta.dirname, "../..");
const meta = JSON.parse(
  readFileSync(path.join(root, "public/geoid/egm96-025deg.json"), "utf8"),
) as GeoidMeta;
const binBytes = readFileSync(path.join(root, "public/geoid/egm96-025deg.bin"));

function stubFetchSuccessAfter(failures: number): void {
  let attempts = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string | URL | Request) => {
      attempts += 1;
      if (attempts <= failures) {
        return { ok: false, json: async () => ({}), arrayBuffer: async () => new ArrayBuffer(0) };
      }
      const href = String(url);
      if (href.includes(".json")) {
        return { ok: true, json: async () => meta, arrayBuffer: async () => new ArrayBuffer(0) };
      }
      return {
        ok: true,
        json: async () => ({}),
        arrayBuffer: async () =>
          binBytes.buffer.slice(binBytes.byteOffset, binBytes.byteOffset + binBytes.byteLength),
      };
    }),
  );
}

describe("geoid lazy load failures", () => {
  beforeEach(() => resetGeoidForTests());
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
    resetGeoidForTests();
  });

  it("does not throw when metadata fetch fails", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: false, json: async () => ({}), arrayBuffer: async () => new ArrayBuffer(0) })),
    );
    await expect(ensureGeoidLoaded()).resolves.toBe(false);
    expect(isGeoidReady()).toBe(false);
  });

  it("retries with backoff then loads MSL grid", async () => {
    vi.useFakeTimers();
    stubFetchSuccessAfter(1);
    const first = await ensureGeoidLoaded();
    expect(first).toBe(false);
    expect(isGeoidReady()).toBe(false);

    await vi.advanceTimersByTimeAsync(1000);
    await vi.waitFor(() => isGeoidReady(), { timeout: 2000 });
    expect(isGeoidReady()).toBe(true);
  });
});