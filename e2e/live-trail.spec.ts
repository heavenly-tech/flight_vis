import fs from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

async function countPaintedCanvasPixels(page: Page, selector: string): Promise<number> {
  return page.evaluate((sel) => {
    const canvas = document.querySelector<HTMLCanvasElement>(sel);
    if (!canvas || canvas.width === 0 || canvas.height === 0) return 0;
    const ctx = canvas.getContext("2d");
    if (!ctx) return 0;
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let painted = 0;
    for (let i = 3; i < data.length; i += 4) {
      if (data[i] > 10) painted += 1;
    }
    return painted;
  }, selector);
}

test.describe("live aircraft trails on admin map and club screen", () => {
  test.setTimeout(60_000);

  test("renders per-flight trails with different effective lengths, syncs club default changes to club screen without altering pilot override", async ({
    browser,
    context,
    page,
    baseURL,
  }) => {
    // Sign in as demo admin in the primary browser context.
    const adminLogin = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(adminLogin.ok()).toBeTruthy();

    // Sign in as demo pilot in a separate API context to own and override N101's flight.
    const pilotCtx = await browser.newContext({ baseURL });

    let n101FlightId: string | null = null;
    let n202FlightId: string | null = null;

    try {
      const pilotLogin = await pilotCtx.request.post("/api/club/login", {
        data: { email: "pilot@demo.test", password: "password12" },
      });
      expect(pilotLogin.ok()).toBeTruthy();

      // Ensure admin sees all tails and club default starts at 5 min.
      const clearTails = await page.request.post("/api/admin/map/tails", {
        data: { tails: [] },
      });
      expect(clearTails.ok()).toBeTruthy();

      const resetDefault = await page.request.patch("/api/admin/club/trail", {
        data: { trailMinutes: 5 },
      });
      expect(resetDefault.ok()).toBeTruthy();

      // Locate the two active demo flights (N101 owned by pilot, N202 owned by admin).
      const pilotFlightsRes = await pilotCtx.request.get("/api/me/flights");
      expect(pilotFlightsRes.ok()).toBeTruthy();
      const pilotFlights = (await pilotFlightsRes.json()) as {
        flights: Array<{ id: string; tail: string; status: string }>;
      };
      n101FlightId =
        pilotFlights.flights.find((f) => f.tail === "N101" && f.status === "active")?.id ?? null;

      const adminFlightsRes = await page.request.get("/api/me/flights");
      expect(adminFlightsRes.ok()).toBeTruthy();
      const adminFlights = (await adminFlightsRes.json()) as {
        flights: Array<{ id: string; tail: string; status: string }>;
      };
      n202FlightId =
        adminFlights.flights.find((f) => f.tail === "N202" && f.status === "active")?.id ?? null;

      expect(n101FlightId, "expected active demo flight for N101").toBeTruthy();
      expect(n202FlightId, "expected active demo flight for N202").toBeTruthy();

      // Set pilot override 2 min on N101, and NULL (use club default = 5 min) on N202.
      const patchN101 = await pilotCtx.request.patch(`/api/flights/${n101FlightId}/trail`, {
        data: { trailMinutes: 2 },
      });
      expect(patchN101.ok()).toBeTruthy();
      const patchN101Body = (await patchN101.json()) as {
        trailMinutes: number | null;
        effectiveTrailMinutes: number;
      };
      expect(patchN101Body.trailMinutes).toBe(2);
      expect(patchN101Body.effectiveTrailMinutes).toBe(2);

      const patchN202 = await page.request.patch(`/api/flights/${n202FlightId}/trail`, {
        data: { trailMinutes: null },
      });
      expect(patchN202.ok()).toBeTruthy();
      const patchN202Body = (await patchN202.json()) as {
        trailMinutes: number | null;
        effectiveTrailMinutes: number;
      };
      expect(patchN202Body.trailMinutes).toBeNull();
      expect(patchN202Body.effectiveTrailMinutes).toBe(5);

      await stubArcgisTiles(page);
      await page.setViewportSize({ width: 1280, height: 800 });
      await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });

      const adminMap = page.locator('[data-testid="admin-map"]');
      const adminTrailN101 = adminMap.locator('[data-testid="live-trail"][data-trail-tail="N101"]');
      const adminTrailN202 = adminMap.locator('[data-testid="live-trail"][data-trail-tail="N202"]');

      await expect(adminTrailN101).toHaveCount(1, { timeout: 15_000 });
      await expect(adminTrailN202).toHaveCount(1, { timeout: 15_000 });
      await expect(adminTrailN101).toHaveAttribute("data-trail-minutes", "2");
      await expect(adminTrailN202).toHaveAttribute("data-trail-minutes", "5");

      const n101PtsInitial = Number(await adminTrailN101.getAttribute("data-trail-points"));
      const n202PtsInitial = Number(await adminTrailN202.getAttribute("data-trail-points"));
      const n101LenInitial = Number(await adminTrailN101.getAttribute("data-trail-length-px"));
      const n202LenInitial = Number(await adminTrailN202.getAttribute("data-trail-length-px"));
      expect(n101PtsInitial).toBeGreaterThanOrEqual(2);
      expect(n202PtsInitial).toBeGreaterThan(n101PtsInitial);
      expect(n101LenInitial).toBeGreaterThan(0);
      expect(n202LenInitial).toBeGreaterThan(n101LenInitial);

      // Verify trails canvas actually painted visible stroke pixels on the admin map.
      const adminPainted = await countPaintedCanvasPixels(
        page,
        '[data-testid="admin-map"] [data-trails-canvas]',
      );
      expect(adminPainted).toBeGreaterThan(20);

      // Open club-screen.html in a second page of the same browser context.
      const screenPage = await context.newPage();
      await stubArcgisTiles(screenPage);
      await screenPage.setViewportSize({ width: 1920, height: 1080 });
      await screenPage.goto("/club-screen.html", { waitUntil: "networkidle" });

      const kioskMap = screenPage.locator('[data-testid="kiosk-map"]');
      const screenTrailN101 = kioskMap.locator('[data-testid="live-trail"][data-trail-tail="N101"]');
      const screenTrailN202 = kioskMap.locator('[data-testid="live-trail"][data-trail-tail="N202"]');

      await expect(screenTrailN101).toHaveCount(1, { timeout: 15_000 });
      await expect(screenTrailN202).toHaveCount(1, { timeout: 15_000 });
      await expect(screenTrailN101).toHaveAttribute("data-trail-minutes", "2");
      await expect(screenTrailN202).toHaveAttribute("data-trail-minutes", "5");

      const screenN101Pts = Number(await screenTrailN101.getAttribute("data-trail-points"));
      const screenN202Pts = Number(await screenTrailN202.getAttribute("data-trail-points"));
      expect(screenN101Pts).toBe(n101PtsInitial);
      expect(screenN202Pts).toBe(n202PtsInitial);

      const screenPainted = await countPaintedCanvasPixels(
        screenPage,
        '[data-testid="kiosk-map"] [data-trails-canvas]',
      );
      expect(screenPainted).toBeGreaterThan(40);

      // Open club settings on admin map and change club default from 5 min -> 15 min.
      await page.locator("#club-settings > summary").click();
      await expect(page.locator("#club-settings")).toHaveAttribute("open", "");
      await expect(page.locator('#club-trail [data-i18n="admin.trailTitle"]')).toHaveText("Estela");

      const chipRow = page.locator('[data-testid="club-trail-chips"]');
      await expect(chipRow.locator("button")).toHaveCount(7);
      await expect(chipRow.locator('button[data-trail-value="5"]')).toHaveAttribute(
        "aria-pressed",
        "true",
      );

      await chipRow.locator('button[data-trail-value="15"]').click();
      await expect(page.locator("#club-trail-msg")).toHaveText("Estela predeterminada guardada.", {
        timeout: 10_000,
      });
      await expect(chipRow.locator('button[data-trail-value="15"]')).toHaveAttribute(
        "aria-pressed",
        "true",
      );

      // Admin map: N202 (non-override) grows to 15 min with more points; N101 (override 2 min) is unchanged!
      await expect(adminTrailN202).toHaveAttribute("data-trail-minutes", "15", { timeout: 10_000 });
      await expect(adminTrailN101).toHaveAttribute("data-trail-minutes", "2");
      const n202PtsAfter15 = Number(await adminTrailN202.getAttribute("data-trail-points"));
      const n101PtsAfter15 = Number(await adminTrailN101.getAttribute("data-trail-points"));
      expect(n202PtsAfter15).toBeGreaterThan(n202PtsInitial);
      expect(n101PtsAfter15).toBe(n101PtsInitial);

      // Club screen (second page): picks up the change automatically via notifyKioskRefetch().
      await expect(screenTrailN202).toHaveAttribute("data-trail-minutes", "15", {
        timeout: 10_000,
      });
      await expect(screenTrailN101).toHaveAttribute("data-trail-minutes", "2");
      const screenN202PtsAfter15 = Number(await screenTrailN202.getAttribute("data-trail-points"));
      const screenN101PtsAfter15 = Number(await screenTrailN101.getAttribute("data-trail-points"));
      expect(screenN202PtsAfter15).toBe(n202PtsAfter15);
      expect(screenN101PtsAfter15).toBe(n101PtsInitial);

      const shotsDir = process.env.SHOTS_DIR;
      if (shotsDir) {
        fs.mkdirSync(shotsDir, { recursive: true });
        await page.locator("#club-trail").scrollIntoViewIfNeeded();
        await page.screenshot({
          path: path.join(shotsDir, "admin-trail-setting-1280x800.png"),
          fullPage: false,
        });
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.screenshot({
          path: path.join(shotsDir, "admin-map-1280x800.png"),
          fullPage: false,
        });
        await screenPage.screenshot({
          path: path.join(shotsDir, "club-screen-1920x1080.png"),
          fullPage: false,
        });
      }

      // Setting club default to Off (0) hides N202's trail while keeping N101's 2-min override.
      await chipRow.locator('button[data-trail-value="0"]').click();
      await expect(chipRow.locator('button[data-trail-value="0"]')).toHaveAttribute(
        "aria-pressed",
        "true",
        { timeout: 10_000 },
      );
      await expect(adminTrailN202).toHaveCount(0, { timeout: 10_000 });
      await expect(adminTrailN101).toHaveCount(1);
      await expect(screenTrailN202).toHaveCount(0, { timeout: 10_000 });
      await expect(screenTrailN101).toHaveCount(1);

      await screenPage.close();
    } finally {
      // Restore demo defaults so repeated runs and other specs stay clean.
      await page.request.patch("/api/admin/club/trail", { data: { trailMinutes: 5 } }).catch(() => {});
      if (n101FlightId) {
        await pilotCtx.request
          .patch(`/api/flights/${n101FlightId}/trail`, { data: { trailMinutes: null } })
          .catch(() => {});
      }
      if (n202FlightId) {
        await page.request
          .patch(`/api/flights/${n202FlightId}/trail`, { data: { trailMinutes: null } })
          .catch(() => {});
      }
      await page.request.post("/api/admin/map/tails", { data: { tails: [] } }).catch(() => {});
      await pilotCtx.close();
    }
  });
});
