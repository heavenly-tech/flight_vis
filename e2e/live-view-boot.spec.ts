import { expect, test } from "@playwright/test";

test.describe.configure({ timeout: 120_000 });

test("live route clears globe preparing status after track loads", async ({ page }) => {
  const fixes = [
    { t: 1_700_000_000_000, lat: -33.4, lon: -70.6, alt: 1200, alt_ref: "wgs84_ellipsoid" },
    { t: 1_700_000_001_000, lat: -33.41, lon: -70.61, alt: 1210, alt_ref: "wgs84_ellipsoid" },
  ];

  await page.addInitScript((payload) => {
    class MockEventSource {
      readyState = 1;
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
      close(): void {}
      private emit(type: string, data: string): void {
        for (const fn of this.listeners.get(type) ?? []) {
          fn({ data } as MessageEvent);
        }
      }
    }
    globalThis.EventSource = MockEventSource as typeof EventSource;

    const auth = { authenticated: true, configured: true };
    const origFetch = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/api/live/sess-boot/fixes")) {
        return new Response(JSON.stringify({ fixes: payload.fixes }), { status: 200 });
      }
      if (url.includes("/api/auth/me")) {
        return new Response(JSON.stringify(auth), { status: 200 });
      }
      return origFetch(input, init);
    };
  }, { fixes });

  await page.goto("/live/sess-boot");
  await expect(page.locator("#flight-name")).toHaveText(/Live session|Sesión en vivo/, {
    timeout: 60_000,
  });
  await expect(page.locator("#status")).toBeHidden({ timeout: 15_000 });
});