import { expect, type Page } from "@playwright/test";
import { PNG } from "pngjs";

/** Query flag that enables the minimal window.__fvE2e surface (no full viewer export). */
export const E2E_GLOBE_HOOKS = "e2eGlobeHooks=1";

/** 1×1 opaque blue PNG (no network). */
const STUB_TILE = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

export function installCspObservers(page: Page): {
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

export async function stubArcgisTiles(page: Page): Promise<void> {
  const fulfillTile = async (route: import("@playwright/test").Route) => {
    await route.fulfill({
      status: 200,
      contentType: "image/png",
      body: STUB_TILE,
    });
  };
  await page.route(/services\.arcgisonline\.com/i, fulfillTile);
  await page.route(/elevation3d\.arcgis\.com/i, async (route) => {
    const method = route.request().method();
    if (method === "GET") {
      await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
      return;
    }
    await route.fulfill({ status: 200, contentType: "application/octet-stream", body: Buffer.alloc(0) });
  });
}

export async function mockAuthAuthenticated(page: Page): Promise<void> {
  await page.route("**/api/auth/me", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ authenticated: true, configured: true, canManageLibrary: true }),
    });
  });
}

export async function mockLiveFixes(
  page: Page,
  sessionId: string,
  fixes: unknown[],
): Promise<void> {
  await page.route(`**/api/live/${sessionId}/fixes`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ fixes }),
    });
  });
}

export function distinctRgbColors(png: PNG, sampleStride = 16): number {
  const seen = new Set<string>();
  const data = png.data;
  for (let y = 0; y < png.height; y += sampleStride) {
    for (let x = 0; x < png.width; x += sampleStride) {
      const i = (png.width * y + x) * 4;
      const key = `${data[i]},${data[i + 1]},${data[i + 2]}`;
      seen.add(key);
      if (seen.size >= 2) return seen.size;
    }
  }
  return seen.size;
}

export async function assertGlobeRenders(
  page: Page,
  consoleErrors: string[],
  pageErrors: string[],
): Promise<void> {
  await expect(page.locator("#status")).toBeHidden({ timeout: 90_000 });
  const canvas = page.locator("#cesiumContainer canvas");
  await expect(canvas).toBeVisible({ timeout: 90_000 });

  await page.waitForFunction(
    () => {
      const e2e = window.__fvE2e;
      const el = document.querySelector("#cesiumContainer canvas") as HTMLCanvasElement | null;
      if (!e2e?.globePainted || !el || el.clientWidth < 10 || el.clientHeight < 10) return false;
      return e2e.getPerfCommands() > 0;
    },
    { timeout: 90_000 },
  );

  const shot = await page.locator("#cesiumContainer").screenshot({ timeout: 30_000 });
  const png = PNG.sync.read(shot);
  expect(png.width).toBeGreaterThan(10);
  expect(png.height).toBeGreaterThan(10);
  expect(distinctRgbColors(png)).toBeGreaterThanOrEqual(2);

  const violations = await page.evaluate(() => {
    if (!window.__cspViolations) {
      throw new Error("missing window.__cspViolations (e2e-csp-hooks.js)");
    }
    return window.__cspViolations;
  });
  expect(violations).toEqual([]);
  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
}

export function hooksUrl(path: string): string {
  const joiner = path.includes("?") ? "&" : "?";
  return `${path}${joiner}${E2E_GLOBE_HOOKS}`;
}