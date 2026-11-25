import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test.describe("admin live map, tail filter persistence, and kiosk links", () => {
  test.setTimeout(60_000);

  test("renders live 2D map at top, empty state when no aircraft, collapsed settings, and preserves unsaved tail edits across 5s poll", async ({
    page,
  }) => {
    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();

    await stubArcgisTiles(page);

    // Isolate per-test persisted tails while forwarding GET/POST to the real backend so
    // parallel --repeat-each workers never race on the shared demo admin row.
    let storedTails: string[] = [];
    await page.route("**/api/admin/map/tails", async (route) => {
      const res = await route.fetch();
      expect(res.ok()).toBeTruthy();
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as { tails?: string[] };
        storedTails = Array.isArray(body.tails) ? body.tails : [];
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ tails: storedTails }),
      });
    });

    let liveMapPollCount = 0;
    await page.route("**/api/admin/map", async (route) => {
      liveMapPollCount += 1;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ aircraft: [] }),
      });
    });

    try {
      await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });

      // Updated tailFilterHint copy
      await expect(page.locator('[data-i18n="admin.tailFilterHint"]')).toHaveText(
        "Las matrículas seleccionadas filtran esta vista y la pantalla del club que actives.",
      );

      // Live 2D map renders at the top with tiles
      const mapEl = page.locator('[data-testid="admin-map"]');
      await expect(mapEl).toBeVisible({ timeout: 15_000 });
      const mapBox = await mapEl.boundingBox();
      expect(mapBox).not.toBeNull();
      expect(mapBox!.width).toBeGreaterThan(200);
      expect(mapBox!.height).toBeGreaterThan(150);
      await expect
        .poll(() => mapEl.locator("img[data-tile]").count(), { timeout: 10_000 })
        .toBeGreaterThan(0);

      // Empty state shows when no aircraft are flying
      const liveEmpty = page.locator("#live-empty");
      await expect(liveEmpty).toBeVisible();
      await expect(liveEmpty).toHaveText("No hay aeronaves en vuelo en este momento.");
      await expect(page.locator("#live li")).toHaveCount(0);

      // Section ordering: map -> tail filter -> kiosk card -> collapsed settings details
      const settingsDetails = page.locator("#club-settings");
      await expect(settingsDetails).toBeVisible();
      await expect(settingsDetails).not.toHaveAttribute("open", "");
      await expect(page.locator("#club-settings > summary")).toHaveText("Configuración del club");

      const mapSectionBox = (await page.locator("#live-map-section").boundingBox())!;
      const tailsSectionBox = (await page.locator("#tail-filter-section").boundingBox())!;
      const kioskCardBox = (await page.locator("#kiosk-card").boundingBox())!;
      const settingsBox = (await settingsDetails.boundingBox())!;
      expect(mapSectionBox.y).toBeLessThan(tailsSectionBox.y);
      expect(tailsSectionBox.y).toBeLessThan(kioskCardBox.y);
      expect(kioskCardBox.y).toBeLessThan(settingsBox.y);

      // Tail checkboxes load and quick buttons work
      const checkboxes = page.locator('#ticks input[type="checkbox"]');
      await expect.poll(() => checkboxes.count(), { timeout: 10_000 }).toBeGreaterThanOrEqual(2);

      await page.locator("#btn-tails-none").click();
      const count = await checkboxes.count();
      for (let i = 0; i < count; i++) {
        await expect(checkboxes.nth(i)).not.toBeChecked();
      }
      await page.locator("#btn-tails-all").click();
      for (let i = 0; i < count; i++) {
        await expect(checkboxes.nth(i)).toBeChecked();
      }

      // Uncheck the first tail and wait > 5s (at least one 5s poll) -> must STAY unchecked!
      const firstCb = checkboxes.first();
      const uncheckedTail = await firstCb.inputValue();
      const pollsBefore = liveMapPollCount;
      await firstCb.uncheck();
      await expect(firstCb).not.toBeChecked();

      await page.waitForTimeout(5_600);
      expect(liveMapPollCount).toBeGreaterThan(pollsBefore);
      await expect(firstCb).not.toBeChecked();

      // Save selection -> shows "Selección guardada." with role="status"
      await page.locator("#save").click();
      const tailsMsg = page.locator("#tails-msg");
      await expect(tailsMsg).toHaveAttribute("role", "status");
      await expect(tailsMsg).toHaveText("Selección guardada.", { timeout: 10_000 });
      await expect(page.locator(`#ticks input[value="${uncheckedTail}"]`)).not.toBeChecked();

      // Reload -> confirm persistence from backend DB
      await page.reload({ waitUntil: "networkidle" });
      await expect(page.locator(`#ticks input[value="${uncheckedTail}"]`)).not.toBeChecked();
      for (let i = 1; i < count; i++) {
        await expect(checkboxes.nth(i)).toBeChecked();
      }
    } finally {
      // Restore demo admin's tail filter so other specs see all tails.
      await page.request.post("/api/admin/map/tails", { data: { tails: [] } });
    }
  });

  test("kiosk card activates display with copy buttons, feedback, and Abrir pantalla link", async ({
    page,
  }) => {
    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();

    await stubArcgisTiles(page);
    await page.route("**/api/admin/map", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          aircraft: [
            {
              tail: "N101",
              pilotName: "Demo Pilot",
              fix: { lat: -33.46, lon: -70.56, alt: 900 },
            },
          ],
        }),
      });
    });

    await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });

    // Live aircraft marker + list render when aircraft is flying
    await expect(page.locator('[data-testid="admin-map"] [data-marker="aircraft"][data-tail="N101"]')).toBeVisible();
    await expect(page.locator("#live li")).toHaveText(["N101 · Demo Pilot"]);
    await expect(page.locator("#live-empty")).toBeHidden();

    // Top "Abrir pantalla del club" link
    const topKioskLink = page.locator("a[data-open-kiosk-link]");
    await expect(topKioskLink).toBeVisible();
    await expect(topKioskLink).toHaveText("Abrir pantalla del club");
    await expect(topKioskLink).toHaveAttribute("href", "/c/demo/screen");
    await expect(topKioskLink).toHaveAttribute("target", "_blank");
    await expect(topKioskLink).toHaveAttribute("rel", "noopener");

    // Dedicated kiosk card activation
    await expect(page.locator("#kiosk-card h2")).toHaveText("Pantalla del club (kiosco)");
    await page.locator("#btn-kiosk").click();

    const urlInput = page.locator("#kiosk-display-url");
    const tokenInput = page.locator("#kiosk-display-token");
    await expect(urlInput).toBeVisible({ timeout: 10_000 });
    await expect(tokenInput).toBeVisible();

    const displayUrl = await urlInput.inputValue();
    const tokenVal = await tokenInput.inputValue();
    expect(displayUrl).toContain("/c/demo/screen");
    expect(tokenVal.length).toBeGreaterThan(8);

    const openDisplayLink = page.locator("#kiosk-open-session");
    await expect(openDisplayLink).toBeVisible();
    await expect(openDisplayLink).toHaveText("Abrir pantalla");
    await expect(openDisplayLink).toHaveAttribute("href", displayUrl);
    await expect(openDisplayLink).toHaveAttribute("target", "_blank");
    await expect(openDisplayLink).toHaveAttribute("rel", "noopener");

    // Copy code button shows "Copiado" feedback in role="status"
    const copyStatus = page.locator("#kiosk-copy-status");
    await expect(copyStatus).toHaveAttribute("role", "status");
    await page.locator("#btn-copy-kiosk-code").click();
    await expect(copyStatus).toHaveText("Copiado");

    // Copy link button also shows "Copiado" feedback
    await page.locator("#btn-copy-kiosk-url").click();
    await expect(copyStatus).toHaveText("Copiado");
  });

  test("Abrir pantalla del club link appears on kiosk layout and checkin QR admin pages", async ({
    page,
  }) => {
    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();

    for (const path of ["/club-admin-kiosk-layout.html", "/club-admin-checkin-qr.html"]) {
      await page.goto(path, { waitUntil: "load" });
      const link = page.locator("a[data-open-kiosk-link]");
      await expect(link, `${path} kiosk link`).toBeVisible({ timeout: 15_000 });
      await expect(link).toHaveText("Abrir pantalla del club");
      await expect(link).toHaveAttribute("href", "/c/demo/screen");
      await expect(link).toHaveAttribute("target", "_blank");
      await expect(link).toHaveAttribute("rel", "noopener");
    }
  });
});
