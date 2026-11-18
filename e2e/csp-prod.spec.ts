import { expect, test } from "@playwright/test";

/** Must match `server-rs/src/static_files.rs` `HTML_CSP` (nginx vhost should mirror this). */
export const PROD_HTML_CSP =
  "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; connect-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

const CSP_VIOLATION =
  /Refused to execute|Refused to load|Content Security Policy|securitypolicyviolation|violates the following Content Security Policy/i;

// TODO(hotfix): drop when separate Cesium `new Function` CSP fix merges — see urgent globe hotfix PR.
const CESIUM_EVAL_PENDING_HOTFIX = /EvalError|unsafe-eval.*evaluat|new Function/i;

function isPendingCesiumHotfix(message: string): boolean {
  return CESIUM_EVAL_PENDING_HOTFIX.test(message);
}

function trackCspViolations(page: import("@playwright/test").Page): string[] {
  const violations: string[] = [];
  page.on("console", (msg) => {
    const text = msg.text();
    if (isPendingCesiumHotfix(text)) return;
    if (CSP_VIOLATION.test(text)) violations.push(text);
  });
  page.on("pageerror", (err) => {
    if (isPendingCesiumHotfix(err.message)) return;
    if (CSP_VIOLATION.test(err.message)) violations.push(err.message);
  });
  void page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      (window as unknown as { __fvCsp?: string[] }).__fvCsp ??= [];
      (window as unknown as { __fvCsp: string[] }).__fvCsp.push(
        `${e.violatedDirective}: ${e.blockedURI || e.sourceFile}`,
      );
    });
  });
  return violations;
}

async function domViolations(page: import("@playwright/test").Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __fvCsp?: string[] }).__fvCsp ?? []);
}

function relevantViolations(events: string[]): string[] {
  return events.filter(
    (v) =>
      !isPendingCesiumHotfix(v) &&
      (v.startsWith("script-src") ||
        v.startsWith("style-src") ||
        v.startsWith("font-src") ||
        CSP_VIOLATION.test(v)),
  );
}

async function expectProdCspHeader(request: import("@playwright/test").APIRequestContext, path: string): Promise<void> {
  const res = await request.get(path);
  expect(res.ok(), `GET ${path}`).toBeTruthy();
  const csp = res.headers()["content-security-policy"];
  expect(csp, `${path} CSP header`).toBe(PROD_HTML_CSP);
}

test.describe.configure({ timeout: 120_000 });

test.describe("production CSP (release server)", () => {
  test("HTML responses use prod CSP and have no inline scripts", async ({ request }) => {
    const paths = [
      "/",
      "/login.html",
      "/public-map.html",
      "/club-map.html",
      "/club-checkin-landing.html",
    ];
    for (const path of paths) {
      await expectProdCspHeader(request, path);
      const html = await (await request.get(path)).text();
      expect(html, `${path} inline script`).not.toMatch(/<script(?![^>]*\ssrc=)/);
      expect(html, `${path} inline handler`).not.toMatch(/\son\w+\s*=/i);
      expect(html, `${path} google fonts`).not.toMatch(/fonts\.googleapis|fonts\.gstatic/i);
    }
  });

  test("missing APK is 404, not SPA HTML", async ({ request }) => {
    const res = await request.get("/app/flightvis.apk");
    expect(res.status()).toBe(404);
    const body = await res.text();
    expect(body).not.toContain("cesiumContainer");
  });

  test("slug page loads without script/style/font CSP violations", async ({ page }) => {
    const violations = trackCspViolations(page);
    await page.goto("/vuelo-5");
    await expect(page.locator("#flight-name")).toBeVisible({ timeout: 60_000 });
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });

  test("club landing page loads fonts/styles under prod CSP", async ({ page, request }) => {
    const violations = trackCspViolations(page);
    await expectProdCspHeader(request, "/club-checkin-landing.html");
    await page.goto("/club-checkin-landing.html");
    await expect(page.locator("h1")).toBeVisible({ timeout: 30_000 });
    const fontOk = await page.evaluate(() => {
      const family = getComputedStyle(document.body).fontFamily.toLowerCase();
      return family.includes("outfit");
    });
    expect(fontOk, "Outfit self-hosted font applied").toBe(true);
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });

  test("/live/{id} chrome loads without script/style/font CSP violations", async ({ page }) => {
    const violations = trackCspViolations(page);
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
    await expect(page.locator("#flight-name")).toHaveText("Live session", { timeout: 60_000 });
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });
});