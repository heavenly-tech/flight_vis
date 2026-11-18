import { expect, test } from "@playwright/test";

function installCspObservers(page: import("@playwright/test").Page): {
  consoleErrors: string[];
  pageErrors: string[];
} {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() !== "error") return;
    const text = msg.text();
    if (/Content Security Policy|EvalError|Refused to evaluate/i.test(text)) {
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (err) => {
    pageErrors.push(String(err));
  });
  return { consoleErrors, pageErrors };
}

async function assertGlobeRenders(
  page: import("@playwright/test").Page,
  consoleErrors: string[],
  pageErrors: string[],
): Promise<void> {
  await expect(page.locator("#status")).toBeHidden({ timeout: 90_000 });
  const canvas = page.locator("#cesiumContainer canvas");
  await expect(canvas).toBeVisible({ timeout: 90_000 });

  await page.waitForFunction(
    () => {
      const fv = (window as unknown as { __fv?: { getPerfStats: () => { commands: number } } }).__fv;
      const el = document.querySelector("#cesiumContainer canvas") as HTMLCanvasElement | null;
      if (!fv || !el || el.clientWidth < 10 || el.clientHeight < 10) return false;
      return fv.getPerfStats().commands > 0;
    },
    { timeout: 90_000 },
  );

  const rendered = await page.evaluate(() => {
    const painted = (window as unknown as { __fvGlobePainted?: boolean }).__fvGlobePainted === true;
    const canvas = document.querySelector("#cesiumContainer canvas") as HTMLCanvasElement;
    const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
    if (!gl) return painted;
    const w = canvas.width;
    const h = canvas.height;
    const samples = [
      [Math.floor(w * 0.25), Math.floor(h * 0.25)],
      [Math.floor(w * 0.75), Math.floor(h * 0.35)],
      [Math.floor(w * 0.5), Math.floor(h * 0.7)],
    ];
    const colors = samples.map(([x, y]) => {
      const buf = new Uint8Array(4);
      gl.readPixels(x, y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, buf);
      return buf.join(",");
    });
    return painted || new Set(colors).size > 1;
  });
  expect(rendered).toBe(true);

  const violations = await page.evaluate(() => {
    return (window as unknown as { __cspViolations?: string[] }).__cspViolations ?? [];
  });
  expect(violations).toEqual([]);
  expect(pageErrors.filter((e) => /EvalError/i.test(e))).toEqual([]);
  expect(consoleErrors).toEqual([]);
}

test.describe("production CSP globe", () => {
  test.setTimeout(120_000);
  test.use({ baseURL: "http://127.0.0.1:9090" });

  test("main map boots Cesium under prod CSP", async ({ page }) => {
    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto("/vuelo-5");
    await assertGlobeRenders(page, consoleErrors, pageErrors);
  });

  test("live map boots Cesium under prod CSP", async ({ page }) => {
    const fixes = [
      { t: 1_700_000_000_000, lat: -33.4, lon: -70.6, alt: 1200, alt_ref: "wgs84_ellipsoid" },
      { t: 1_700_000_001_000, lat: -33.41, lon: -70.61, alt: 1210, alt_ref: "wgs84_ellipsoid" },
    ];

    await page.route("**/api/live/sess-csp/fixes", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ fixes }),
      });
    });
    await page.route("**/api/auth/me", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ authenticated: true, configured: true }),
      });
    });
    await page.route("**/*", async (route) => {
      const req = route.request();
      if (req.resourceType() !== "document") {
        await route.continue();
        return;
      }
      const response = await route.fetch();
      let html = await response.text();
      const hook = '<script src="/e2e-live-csp-sse.js"></script>';
      if (!html.includes("e2e-live-csp-sse.js")) {
        html = html.replace("<head>", `<head>${hook}`);
      }
      await route.fulfill({
        response,
        body: html,
        headers: {
          ...response.headers(),
          "content-security-policy": response.headers()["content-security-policy"] ?? "",
        },
      });
    });

    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto("/live/sess-csp");
    await expect(page.locator("#flight-name")).toHaveText("Live session", { timeout: 15_000 });
    await assertGlobeRenders(page, consoleErrors, pageErrors);
  });
});