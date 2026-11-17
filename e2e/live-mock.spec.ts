import { expect, test } from "@playwright/test";

test("live page receives mocked SSE fixes", async ({ page }) => {
  const fixes = [
    { t: 1_700_000_000_000, lat: -33.4, lon: -70.6, alt: 1200, alt_ref: "wgs84_ellipsoid" },
    { t: 1_700_000_001_000, lat: -33.41, lon: -70.61, alt: 1210, alt_ref: "wgs84_ellipsoid" },
  ];

  await page.addInitScript((payload) => {
    class MockEventSource {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSED = 2;
      readyState = MockEventSource.OPEN;
      url: string;
      onerror: (() => void) | null = null;
      private listeners = new Map<string, Array<(ev: MessageEvent) => void>>();
      constructor(url: string) {
        this.url = url;
        queueMicrotask(() => {
          for (const fix of payload.fixes) {
            this.emit("fix", JSON.stringify(fix));
          }
        });
      }
      addEventListener(type: string, fn: (ev: MessageEvent) => void): void {
        const list = this.listeners.get(type) ?? [];
        list.push(fn);
        this.listeners.set(type, list);
      }
      close(): void {
        this.readyState = MockEventSource.CLOSED;
      }
      private emit(type: string, data: string): void {
        for (const fn of this.listeners.get(type) ?? []) {
          fn({ data } as MessageEvent);
        }
      }
    }
    globalThis.EventSource = MockEventSource as typeof EventSource;
  }, { fixes });

  await page.goto("/e2e-live.html?id=mock-session");
  await expect(page.locator("#live-status")).toHaveText("fixes:2", { timeout: 5000 });
  await expect(page.locator("#live-count")).toHaveText("2");
});