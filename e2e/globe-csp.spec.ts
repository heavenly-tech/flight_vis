import { expect, test } from "@playwright/test";
import {
  assertGlobeRenders,
  hooksUrl,
  installCspObservers,
  mockAuthAuthenticated,
  mockLiveFixes,
  stubArcgisTiles,
} from "./globe-csp-helpers";

const LIVE_FIXES = [
  { t: 1_700_000_000_000, lat: -33.4, lon: -70.6, alt: 1200, alt_ref: "wgs84_ellipsoid" },
  { t: 1_700_000_001_000, lat: -33.41, lon: -70.61, alt: 1210, alt_ref: "wgs84_ellipsoid" },
];

test.describe("production CSP globe", () => {
  test.describe.configure({ mode: "serial" });
  test.setTimeout(120_000);

  test.beforeEach(async ({ page }) => {
    await stubArcgisTiles(page);
  });

  test("root loads default flight under prod CSP", async ({ page }) => {
    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto(hooksUrl("/"));
    await expect(page.locator("#flight-name")).not.toHaveText("Loading…", { timeout: 90_000 });
    await assertGlobeRenders(page, consoleErrors, pageErrors);
  });

  test("slug map /vuelo-5 under prod CSP", async ({ page }) => {
    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto(hooksUrl("/vuelo-5"));
    await assertGlobeRenders(page, consoleErrors, pageErrors);
  });

  test("live map /live/sess-csp under prod CSP", async ({ page }) => {
    await mockAuthAuthenticated(page);
    await mockLiveFixes(page, "sess-csp", LIVE_FIXES);

    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto(hooksUrl("/live/sess-csp"));
    await expect(page.locator("#flight-name")).toHaveText("Live session", { timeout: 15_000 });
    await assertGlobeRenders(page, consoleErrors, pageErrors);
  });

  test("playback advances aircraft entity under prod CSP", async ({ page }) => {
    const { consoleErrors, pageErrors } = installCspObservers(page);
    await page.goto(hooksUrl("/vuelo-5"));
    await assertGlobeRenders(page, consoleErrors, pageErrors);

    const start = await page.evaluate(() => {
      const pos = window.__fvE2e?.getAircraftPosition();
      if (!pos) throw new Error("missing aircraft position before play");
      return pos;
    });

    await page.locator("#btn-play").click();
    await page.waitForFunction(
      (origin) => {
        const pos = window.__fvE2e?.getAircraftPosition();
        if (!pos) return false;
        return Math.hypot(pos.lon - origin.lon, pos.lat - origin.lat) > 1e-6;
      },
      start,
      { timeout: 60_000 },
    );

    expect(pageErrors.filter((e) => /EvalError/i.test(e))).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });
});