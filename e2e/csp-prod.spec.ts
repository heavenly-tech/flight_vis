import { expect, test } from "@playwright/test";
import {
  expectedCspForPath,
  HTML_GLOBE_CSP,
  HTML_MAP_CSP,
  HTML_STRICT_CSP,
  HTML_CSP_ROUTE_TABLE,
  policyKindForPath,
} from "./csp-constants";
import { stubArcgisTiles, installCspObservers } from "./globe-csp-helpers";

const CSP_VIOLATION =
  /Refused to execute|Refused to load|Content Security Policy|securitypolicyviolation|violates the following Content Security Policy/i;

function assertStrictCsp(csp: string, path: string) {
  expect(csp, `${path} strict CSP`).toBe(HTML_STRICT_CSP);
  expect(csp).toContain("img-src 'self';");
  expect(csp).not.toContain("wasm-unsafe-eval");
  expect(csp).not.toMatch(/https?:\/\//);
  expect(csp).not.toContain("blob:");
}

function assertMapCsp(csp: string, path: string) {
  expect(csp, `${path} map CSP`).toBe(HTML_MAP_CSP);
  expect(csp).toContain("img-src 'self' https://services.arcgisonline.com");
  expect(csp).not.toContain("wasm-unsafe-eval");
  expect(csp).not.toContain("elevation3d.arcgis.com");
  expect(csp).not.toContain("blob:");
}

function assertGlobeCsp(csp: string, path: string) {
  expect(csp, `${path} globe CSP`).toBe(HTML_GLOBE_CSP);
  expect(csp).toContain("wasm-unsafe-eval");
  expect(csp).toContain("https://services.arcgisonline.com");
}

function trackCspViolations(page: import("@playwright/test").Page): {
  violations: string[];
  pageErrors: string[];
} {
  const violations: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (msg) => {
    const text = msg.text();
    if (CSP_VIOLATION.test(text)) violations.push(text);
  });
  page.on("pageerror", (err) => {
    pageErrors.push(String(err));
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
  return { violations, pageErrors };
}

async function domViolations(page: import("@playwright/test").Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __fvCsp?: string[] }).__fvCsp ?? []);
}

function relevantViolations(events: string[]): string[] {
  return events.filter(
    (v) =>
      v.startsWith("script-src") ||
      v.startsWith("style-src") ||
      v.startsWith("font-src") ||
      CSP_VIOLATION.test(v),
  );
}

async function expectCspHeader(
  request: import("@playwright/test").APIRequestContext,
  path: string,
): Promise<void> {
  const res = await request.get(path);
  expect(res.ok(), `GET ${path}`).toBeTruthy();
  const csp = res.headers()["content-security-policy"] ?? "";
  const expected = expectedCspForPath(path);
  expect(csp, `${path} CSP header`).toBe(expected);
}

function expectNoReferrer(headers: Record<string, string>, label: string) {
  expect(headers["referrer-policy"] ?? headers["Referrer-Policy"], label).toBe("no-referrer");
}

test.describe.configure({ timeout: 120_000 });

test.describe("production CSP (release server)", () => {
  test("every HTML CSP route resolves to strict, map, or globe policy", () => {
    for (const path of Object.keys(HTML_CSP_ROUTE_TABLE)) {
      expect(policyKindForPath(path), path).toBeDefined();
      expectedCspForPath(path);
    }
    expect(policyKindForPath("/club-screen.html")).toBe("map");
    expect(policyKindForPath("/public-map.html")).toBe("map");
    expect(policyKindForPath("/club-admin-map.html")).toBe("map");
    expect(policyKindForPath("/vuelo-5")).toBe("globe");
    expect(policyKindForPath("/assets/foo.js")).toBeUndefined();
  });

  test("strict CSP on login, signup, account, and admin HTML", async ({ request }) => {
    for (const signupPath of ["/signup", "/signup.html"]) {
      const res = await request.get(signupPath, { maxRedirects: 0 });
      expect(res.status(), signupPath).toBe(200);
      assertStrictCsp(res.headers()["content-security-policy"] ?? "", signupPath);
      const html = await res.text();
      expect(html, signupPath).toContain('data-page="account-signup"');
    }
    const strictPaths = [
      "/login",
      "/login.html",
      "/account-clubs.html",
      "/club-admin-types.html",
      "/club-admin-requests.html",
      "/club-checkin-landing.html",
      "/club-map.html",
    ];
    for (const path of strictPaths) {
      const res = await request.get(path);
      expect(res.ok(), path).toBeTruthy();
      assertStrictCsp(res.headers()["content-security-policy"] ?? "", path);
    }
  });

  test("map CSP on tile-map pages while login remains strict", async ({ page, request }) => {
    for (const path of ["/club-screen.html", "/public-map.html"]) {
      const res = await request.get(path);
      expect(res.ok(), path).toBeTruthy();
      assertMapCsp(res.headers()["content-security-policy"] ?? "", path);
    }

    const loginRes = await request.get("/login.html");
    expect(loginRes.ok(), "/login.html").toBeTruthy();
    const loginCsp = loginRes.headers()["content-security-policy"] ?? "";
    assertStrictCsp(loginCsp, "/login.html");
    expect(loginCsp).not.toContain("services.arcgisonline.com");

    const unauthAdminMapRes = await request.get("/club-admin-map.html", { maxRedirects: 0 });
    expect(unauthAdminMapRes.status(), "unauthenticated /club-admin-map.html").toBe(302);
    expect(
      unauthAdminMapRes.headers()["location"] ?? "",
      "unauthenticated /club-admin-map.html redirect",
    ).toMatch(/^\/login\?next=/);

    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok(), "POST /api/club/login").toBeTruthy();

    const adminMapRes = await page.request.get("/club-admin-map.html", { maxRedirects: 0 });
    expect(adminMapRes.status(), "/club-admin-map.html").toBe(200);
    expect(
      adminMapRes.headers()["location"],
      "/club-admin-map.html must not redirect",
    ).toBeUndefined();
    assertMapCsp(
      adminMapRes.headers()["content-security-policy"] ?? "",
      "/club-admin-map.html",
    );

    const adminPilotsRes = await page.request.get("/club-admin-pilots.html", { maxRedirects: 0 });
    expect(adminPilotsRes.status(), "/club-admin-pilots.html").toBe(200);
    expect(
      adminPilotsRes.headers()["location"],
      "/club-admin-pilots.html must not redirect",
    ).toBeUndefined();
    assertStrictCsp(
      adminPilotsRes.headers()["content-security-policy"] ?? "",
      "/club-admin-pilots.html",
    );
  });

  test("admin HTML strict CSP with staff session", async ({ page }) => {
    await page.goto("/login?mode=password&next=/club-admin-types.html");
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#submit-password").click();
    await page.waitForURL(/club-admin-types\.html/, { timeout: 30_000 });
    const res = await page.goto("/club-admin-types.html");
    expect(res?.ok()).toBeTruthy();
    assertStrictCsp(res?.headers()["content-security-policy"] ?? "", "/club-admin-types.html");
  });

  test("globe CSP on viewer HTML and share template", async ({ request }) => {
    const globePaths = ["/", "/vuelo-5", "/share-flight.html"];
    for (const path of globePaths) {
      const res = await request.get(path);
      expect(res.ok(), path).toBeTruthy();
      assertGlobeCsp(res.headers()["content-security-policy"] ?? "", path);
    }
  });

  test("dynamic club and share HTML headers", async ({ request }) => {
    const demoToken = "00000000-0000-0000-0000-000000000001";
    const rows: { path: string; csp: "strict" | "map" | "globe"; noReferrer?: boolean }[] = [
      { path: "/c/demo", csp: "strict", noReferrer: true },
      { path: "/c/demo/registro", csp: "strict", noReferrer: true },
      { path: `/c/demo/q/${demoToken}`, csp: "strict", noReferrer: true },
      { path: "/s/share-e2e-token", csp: "globe", noReferrer: true },
      { path: "/club/screen", csp: "map", noReferrer: true },
      { path: "/c/demo/screen", csp: "map", noReferrer: true },
    ];
    for (const row of rows) {
      const res = await request.get(row.path);
      expect(res.status() !== 500, row.path).toBeTruthy();
      const csp = res.headers()["content-security-policy"] ?? "";
      if (row.csp === "strict") assertStrictCsp(csp, row.path);
      else if (row.csp === "map") assertMapCsp(csp, row.path);
      else assertGlobeCsp(csp, row.path);
      if (row.noReferrer) expectNoReferrer(res.headers(), row.path);
    }
  });

  test("magic confirm HTML uses strict CSP and no-referrer", async ({ request }) => {
    for (const path of [
      "/api/account/auth/magic?token=e2e-test-token",
      "/api/club/auth/magic?token=e2e-test-token",
    ]) {
      const res = await request.get(path, { headers: { Accept: "text/html" } });
      expect(res.ok(), path).toBeTruthy();
      assertStrictCsp(res.headers()["content-security-policy"] ?? "", path);
      expectNoReferrer(res.headers(), path);
    }
  });

  test("HTML responses have no inline scripts", async ({ request }) => {
    const paths = ["/", "/login", "/public-map.html", "/club-map.html", "/club-checkin-landing.html"];
    for (const path of paths) {
      await expectCspHeader(request, path);
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
    const { violations, pageErrors } = trackCspViolations(page);
    await page.goto("/vuelo-5");
    await expect(page.locator("#flight-name")).toBeVisible({ timeout: 60_000 });
    expect(pageErrors, "uncaught page errors").toEqual([]);
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });

  test("share flight page loads under globe CSP without violations", async ({ page }) => {
    await stubArcgisTiles(page);
    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto("/s/e2e-share-token");
    await expect(page.locator("#cesiumContainer")).toBeVisible({ timeout: 60_000 });
    expect(pageErrors.filter((e) => /EvalError/i.test(e)), "EvalError").toEqual([]);
    expect(relevantViolations(consoleErrors), "console CSP violations").toEqual([]);
  });

  test("club landing page loads fonts/styles under strict CSP", async ({ page, request }) => {
    const { violations, pageErrors } = trackCspViolations(page);
    await expectCspHeader(request, "/club-checkin-landing.html");
    await page.goto("/club-checkin-landing.html");
    await expect(page.locator("main")).toBeVisible({ timeout: 30_000 });
    const fontOk = await page.evaluate(() => {
      const family = getComputedStyle(document.body).fontFamily.toLowerCase();
      return family.includes("outfit");
    });
    expect(fontOk, "Outfit self-hosted font applied").toBe(true);
    expect(pageErrors, "uncaught page errors").toEqual([]);
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });

  test("/live/{id} chrome loads without script/style/font CSP violations", async ({ page }) => {
    const { violations, pageErrors } = trackCspViolations(page);
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

      const auth = { authenticated: true, configured: true, canManageLibrary: true };
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
    await expect(page.locator("#flight-name")).toHaveText(/Live session|Sesión en vivo/, {
      timeout: 60_000,
    });
    expect(pageErrors, "uncaught page errors").toEqual([]);
    expect(relevantViolations(violations), "console CSP violations").toEqual([]);
    expect(relevantViolations(await domViolations(page)), "securitypolicyviolation events").toEqual([]);
  });
});