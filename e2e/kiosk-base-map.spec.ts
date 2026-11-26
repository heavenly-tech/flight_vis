import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.setTimeout(60_000);

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

async function loginDemoAdmin(page: import("@playwright/test").Page) {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
}

test("kiosk screen shows map with base view, club name, and empty state when no aircraft", async ({
  page,
}) => {
  await loginDemoAdmin(page);
  await stubArcgisTiles(page);

  await page.route("**/api/display/map", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ aircraft: [] }),
    });
  });

  await page.goto("/club-screen.html");

  await expect(page.locator("#activate")).toHaveCount(0);
  await expect(page.locator("#club-name")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#club-name")).not.toHaveText("");

  await expect(page.locator("#map-empty")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#map-empty")).toHaveText("No hay aeronaves en vuelo.");
  await expect(page.locator("#map li")).toHaveCount(0);

  const kioskMap = page.locator('[data-testid="kiosk-map"]');
  await expect(kioskMap).toBeVisible({ timeout: 15_000 });
  await expect
    .poll(() => kioskMap.locator("img[data-tile]").count(), { timeout: 15_000 })
    .toBeGreaterThan(0);
  await expect(kioskMap).toHaveAttribute("data-view", /^(base|fallback)$/);
});

test("legacy club-globe-screen URLs redirect to club-screen", async ({ page }) => {
  await loginDemoAdmin(page);
  await stubArcgisTiles(page);

  await page.goto("/club-globe-screen.html");
  await expect(page).toHaveURL(/\/club-screen\.html$/);
  await expect(page.locator(".kiosk-header h2")).toHaveText("Aeronaves en vivo");
  await expect(page.locator("#activate")).toHaveCount(0);
  await expect(page.locator("#activate-token")).toHaveCount(0);
});

test("kiosk at 390x844 clamps QR inside viewport, focuses tail on tap, shows details, and resets with Ver todas", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await loginDemoAdmin(page);
  await stubArcgisTiles(page);

  // Stub wide 16:9 default layout to verify 390px clamping does not clip QR off the right edge.
  await page.route("**/api/display/kiosk-layout", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ x: 0.85625, y: 0.76, size: 0.22, checkinQrVisible: true }),
    });
  });

  await page.route("**/api/display/base", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        icao: "SCTB",
        lat: -33.456,
        lon: -70.548,
        clubName: "Club Aéreo de Santiago",
      }),
    });
  });

  let emptyMode = false;
  await page.route("**/api/display/map", async (route) => {
    const aircraft = emptyMode
      ? []
      : [
          {
            tail: "CC-AAA",
            pilotName: "Piloto Uno",
            fix: { lat: -33.42, lon: -70.54, alt: 1120, speed: 42 },
          },
          {
            tail: "CC-BBB",
            pilotName: "Piloto Dos",
            fix: { lat: -33.52, lon: -70.61 },
          },
        ];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ aircraft }),
    });
  });

  await page.goto("/club-screen.html");

  await expect(page.locator("#activate")).toHaveCount(0);
  await expect(page.locator("#club-name")).toHaveText("Club Aéreo de Santiago");
  await expect(page.locator("#map-empty")).toBeHidden();

  // QR bounding box must stay completely inside the 390x844 viewport.
  const qrWrap = page.locator("#qr-wrap");
  await expect(qrWrap).toBeVisible({ timeout: 10_000 });
  const qrBox = (await qrWrap.boundingBox())!;
  expect(qrBox).not.toBeNull();
  expect(qrBox.x).toBeGreaterThanOrEqual(0);
  expect(qrBox.y).toBeGreaterThanOrEqual(0);
  expect(qrBox.x + qrBox.width).toBeLessThanOrEqual(390.5);
  expect(qrBox.y + qrBox.height).toBeLessThanOrEqual(844.5);
  expect(qrBox.width).toBeLessThanOrEqual(390 * 0.28 + 1);

  // Aircraft markers must not overlap header or Ocultar QR button.
  const headerBox = (await page.locator(".kiosk-header").boundingBox())!;
  const qrBtnBox = (await page.locator("#btn-local-qr").boundingBox())!;
  const markerA = page.locator('[data-marker="aircraft"][data-tail="CC-AAA"]');
  await expect(markerA).toBeVisible({ timeout: 10_000 });
  const markerABox = (await markerA.boundingBox())!;
  expect(markerABox.y).toBeGreaterThanOrEqual(headerBox.y + headerBox.height - 1);
  expect(markerABox.y).toBeGreaterThanOrEqual(qrBtnBox.y + qrBtnBox.height - 1);

  // Each live aircraft list item is a >=44px touch-target button.
  const chipA = page.locator('#map li button[data-tail="CC-AAA"]');
  await expect(chipA).toBeVisible();
  await expect(chipA).toHaveAttribute("aria-pressed", "false");
  const chipABox = (await chipA.boundingBox())!;
  expect(chipABox.height).toBeGreaterThanOrEqual(44);

  // Tap tail chip -> focuses map, highlights chip, displays detail strip and Ver todas.
  await chipA.click();
  const kioskMap = page.locator('[data-testid="kiosk-map"]');
  await expect(kioskMap).toHaveAttribute("data-view", "focus");
  await expect(kioskMap).toHaveAttribute("data-focused-tail", "CC-AAA");
  await expect(chipA).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#focus-detail")).toBeVisible();
  await expect(page.locator("#focus-detail")).toContainText("CC-AAA");
  await expect(page.locator("#focus-detail")).toContainText("Piloto Uno");
  await expect(page.locator("#focus-detail")).toContainText("1120 m");
  await expect(page.locator("#focus-detail")).toContainText("151 km/h");

  const viewAllBtn = page.locator("#btn-view-all");
  await expect(viewAllBtn).toBeVisible();
  await expect(viewAllBtn).toHaveText("Ver todas");

  // Click Ver todas -> resets back to fit-all view.
  await viewAllBtn.click();
  await expect(kioskMap).toHaveAttribute("data-view", "flights");
  await expect(chipA).toHaveAttribute("aria-pressed", "false");
  await expect(viewAllBtn).toBeHidden();
  await expect(page.locator("#focus-detail")).toBeHidden();

  // Clicking a map marker directly also focuses that tail.
  const markerB = page.locator('[data-marker="aircraft"][data-tail="CC-BBB"]');
  await expect(markerB).toBeVisible();
  await markerB.click();
  await expect(kioskMap).toHaveAttribute("data-view", "focus");
  await expect(kioskMap).toHaveAttribute("data-focused-tail", "CC-BBB");
  await expect(page.locator("#focus-detail")).toContainText("CC-BBB");
  await expect(page.locator("#focus-detail")).toContainText("Piloto Dos");

  // When aircraft disappear on next poll, focus clears and empty state appears.
  emptyMode = true;
  await expect(page.locator("#map-empty")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#map-empty")).toHaveText("No hay aeronaves en vuelo.");
  await expect(viewAllBtn).toBeHidden();
  await expect(page.locator("#focus-detail")).toBeHidden();
});

test("admin can set and get club base aerodrome", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const getRes = await page.request.get("/api/admin/club/base");
  expect(getRes.ok()).toBeTruthy();
  const getData = await getRes.json();
  expect(getData).toHaveProperty("icao");
  expect(getData).toHaveProperty("lat");
  expect(getData).toHaveProperty("lon");

  const patchRes = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: -33.456, lon: -70.548 },
  });
  expect(patchRes.ok()).toBeTruthy();
  const patchData = await patchRes.json();
  expect(patchData.icao).toBe("SCTB");
  expect(patchData.lat).toBe(-33.456);
  expect(patchData.lon).toBe(-70.548);

  const getRes2 = await page.request.get("/api/admin/club/base");
  expect(getRes2.ok()).toBeTruthy();
  const getData2 = await getRes2.json();
  expect(getData2.icao).toBe("SCTB");
  expect(getData2.lat).toBe(-33.456);
  expect(getData2.lon).toBe(-70.548);
});

test("admin cannot set invalid base aerodrome", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const patchRes1 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SC", lat: -33.456, lon: -70.548 },
  });
  expect(patchRes1.status()).toBe(400);
  expect((await patchRes1.json()).error).toBe("invalid icao");

  const patchRes2 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: 95, lon: -70.548 },
  });
  expect(patchRes2.status()).toBe(400);
  expect((await patchRes2.json()).error).toBe("invalid lat");

  const patchRes3 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: -33.456, lon: -181 },
  });
  expect(patchRes3.status()).toBe(400);
  expect((await patchRes3.json()).error).toBe("invalid lon");
});

test("display base endpoint returns club base and clubName", async ({ page }) => {
  await loginDemoAdmin(page);

  const baseRes = await page.request.get("/api/display/base");
  expect(baseRes.ok()).toBeTruthy();
  const baseData = await baseRes.json();
  expect(baseData).toHaveProperty("icao");
  expect(baseData).toHaveProperty("lat");
  expect(baseData).toHaveProperty("lon");
  expect(typeof baseData.clubName).toBe("string");
  expect(baseData.clubName.length).toBeGreaterThan(0);
});
