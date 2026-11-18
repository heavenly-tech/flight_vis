import { expect, test } from "@playwright/test";

const CSP_VIOLATION = /Refused to execute|Content Security Policy|securitypolicyviolation/i;

test.describe("production CSP (release server)", () => {
  test("built HTML has no inline scripts", async ({ request }) => {
    const paths = [
      "/",
      "/login.html",
      "/public-map.html",
      "/club-map.html",
      "/club-checkin-landing.html",
    ];
    for (const path of paths) {
      const res = await request.get(path);
      expect(res.ok(), `GET ${path}`).toBeTruthy();
      const html = await res.text();
      expect(html, `${path} inline script`).not.toMatch(/<script(?![^>]*\ssrc=)/);
      expect(html, `${path} inline handler`).not.toMatch(/\son\w+\s*=/i);
    }
  });

  test("/live/{id} prefill and status hidden without CSP violations", async ({ page }) => {
    const violations: string[] = [];
    page.on("console", (msg) => {
      const text = msg.text();
      if (CSP_VIOLATION.test(text)) violations.push(text);
    });
    page.on("pageerror", (err) => {
      if (CSP_VIOLATION.test(err.message)) violations.push(err.message);
    });
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (e) => {
        // Playwright exposes violations via the event; stash for assertion.
        (window as unknown as { __fvCsp?: string[] }).__fvCsp ??= [];
        (window as unknown as { __fvCsp: string[] }).__fvCsp.push(
          `${e.violatedDirective}: ${e.blockedURI}`,
        );
      });
    });

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
        if (url.includes("/api/live/sess-csp/fixes")) {
          return new Response(JSON.stringify({ fixes: payload.fixes }), { status: 200 });
        }
        if (url.includes("/api/auth/me")) {
          return new Response(JSON.stringify(auth), { status: 200 });
        }
        return origFetch(input, init);
      };
    }, { fixes });

    await page.goto("/live/sess-csp");
    await expect(page.locator("#flight-name")).toHaveText("Live session", { timeout: 15_000 });
    await expect(page.locator("#status")).toBeHidden({ timeout: 15_000 });

    const domViolations = await page.evaluate(() => (window as unknown as { __fvCsp?: string[] }).__fvCsp ?? []);
    expect(violations, "console CSP violations").toEqual([]);
    expect(domViolations, "securitypolicyviolation events").toEqual([]);
  });
});