import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  KIOSK_SYNC_CHANNEL,
  KIOSK_SYNC_STORAGE_KEY,
  notifyKioskLogout,
  notifyKioskRefetch,
  subscribeKioskSignals,
  type KioskSyncSignal,
} from "./kioskSync";

class MockBroadcastChannel {
  static channels = new Map<string, Set<MockBroadcastChannel>>();
  static posted: Array<{ channel: string; data: unknown }> = [];
  readonly name: string;
  private listeners = new Set<(ev: MessageEvent) => void>();
  closed = false;

  constructor(name: string) {
    this.name = name;
    const set = MockBroadcastChannel.channels.get(name) ?? new Set();
    set.add(this);
    MockBroadcastChannel.channels.set(name, set);
  }

  addEventListener(type: string, fn: (ev: MessageEvent) => void): void {
    if (type === "message") this.listeners.add(fn);
  }

  removeEventListener(type: string, fn: (ev: MessageEvent) => void): void {
    if (type === "message") this.listeners.delete(fn);
  }

  postMessage(data: unknown): void {
    MockBroadcastChannel.posted.push({ channel: this.name, data });
    const peers = MockBroadcastChannel.channels.get(this.name) ?? new Set();
    for (const peer of peers) {
      if (peer === this || peer.closed) continue;
      for (const fn of peer.listeners) {
        fn({ data } as MessageEvent);
      }
    }
  }

  close(): void {
    this.closed = true;
    MockBroadcastChannel.channels.get(this.name)?.delete(this);
  }
}

describe("kioskSync cross-window signal helper", () => {
  beforeEach(() => {
    MockBroadcastChannel.channels.clear();
    MockBroadcastChannel.posted = [];
    localStorage.clear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it("broadcasts signal-only refetch and logout messages on BroadcastChannel with no data payload", () => {
    vi.stubGlobal("BroadcastChannel", MockBroadcastChannel);
    const received: KioskSyncSignal[] = [];
    const unsubscribe = subscribeKioskSignals((sig) => received.push(sig));

    notifyKioskRefetch();
    notifyKioskLogout();

    expect(received).toEqual(["refetch", "logout"]);
    expect(MockBroadcastChannel.posted).toEqual([
      { channel: KIOSK_SYNC_CHANNEL, data: { type: "refetch" } },
      { channel: KIOSK_SYNC_CHANNEL, data: { type: "logout" } },
    ]);
    // Every posted payload must strictly have only the `type` key (no club/map/QR data).
    for (const entry of MockBroadcastChannel.posted) {
      expect(Object.keys(entry.data as Record<string, unknown>)).toEqual(["type"]);
    }

    // Storage ping only holds "<signal>:<timestamp>:<nonce>".
    const stored = localStorage.getItem(KIOSK_SYNC_STORAGE_KEY) ?? "";
    expect(stored).toMatch(/^logout:\d+:\d+-[a-z0-9]+$/i);

    unsubscribe();
    notifyKioskRefetch();
    expect(received).toEqual(["refetch", "logout"]);
  });

  it("falls back to storage events when BroadcastChannel is unavailable and ignores unrelated keys or invalid payloads", () => {
    vi.stubGlobal("BroadcastChannel", undefined);
    const received: KioskSyncSignal[] = [];
    const unsubscribe = subscribeKioskSignals((sig) => received.push(sig));

    notifyKioskRefetch();
    const refetchPing = localStorage.getItem(KIOSK_SYNC_STORAGE_KEY);
    expect(refetchPing).toMatch(/^refetch:\d+:/);

    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: refetchPing,
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: "other-key",
        newValue: "logout:1:1",
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "unknown:1:1",
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "logout:12345:2-abc",
      }),
    );

    expect(received).toEqual(["refetch", "logout"]);
    unsubscribe();
  });

  it("ignores non-signal messages on BroadcastChannel", () => {
    vi.stubGlobal("BroadcastChannel", MockBroadcastChannel);
    const received: KioskSyncSignal[] = [];
    const unsubscribe = subscribeKioskSignals((sig) => received.push(sig));

    const rogue = new MockBroadcastChannel(KIOSK_SYNC_CHANNEL);
    rogue.postMessage("refetch");
    rogue.postMessage({ type: "other" });
    rogue.postMessage(null);
    rogue.postMessage({ type: "refetch" });
    rogue.close();

    expect(received).toEqual(["refetch"]);
    unsubscribe();
  });
});
