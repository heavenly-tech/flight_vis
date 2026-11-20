import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.setTimeout(60_000);

test("kiosk globe screen shows map with base view when no aircraft", async ({ page }) => {
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  // Create a display session
  const displayRes = await page.request.post("/api/admin/display-sessions", {
    data: { label: "e2e" },
  });
  expect(displayRes.ok()).toBeTruthy();
  const displayData = await displayRes.json();
  const token = displayData.token;

  // Stub ArcGIS tiles
  await stubArcgisTiles(page);

  // Stub display/map to return empty aircraft
  await page.route("/api/display/map", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ aircraft: [] }),
    });
  });

  // Open the globe screen
  await page.goto("/club-globe-screen.html", { waitUntil: "networkidle" });

  // Activate the display
  await page.fill("#activate-token", token);
  await page.click("#btn-activate");

  // Wait for activation
  await expect(page.locator("#activate")).toBeHidden({ timeout: 10000 });

  // Check that the map container is visible
  const kioskMap = page.locator("[data-testid=\"kiosk-map\"]");
  await expect(kioskMap).toBeVisible({ timeout: 10000 });

  // Check that at least one tile image is present
  const tileImages = kioskMap.locator("img[data-tile]");
  await expect(tileImages).toHaveCountGreaterThan(0, { timeout: 10000 });

  // Check data-view attribute (should be base or fallback)
  const viewType = await kioskMap.getAttribute("data-view");
  expect(viewType).toMatch(/^(base|fallback)$/);
});

test("admin can set and get club base aerodrome", async ({ page }) => {
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  // Get current base
  const getRes = await page.request.get("/api/admin/club/base");
  expect(getRes.ok()).toBeTruthy();
  const getData = await getRes.json();
  expect(getData).toHaveProperty("icao");
  expect(getData).toHaveProperty("lat");
  expect(getData).toHaveProperty("lon");

  // Set new base (SCTB)
  const patchRes = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: -33.456, lon: -70.548 },
  });
  expect(patchRes.ok()).toBeTruthy();
  const patchData = await patchRes.json();
  expect(patchData.icao).toBe("SCTB");
  expect(patchData.lat).toBe(-33.456);
  expect(patchData.lon).toBe(-70.548);

  // Verify it was saved
  const getRes2 = await page.request.get("/api/admin/club/base");
  expect(getRes2.ok()).toBeTruthy();
  const getData2 = await getRes2.json();
  expect(getData2.icao).toBe("SCTB");
  expect(getData2.lat).toBe(-33.456);
  expect(getData2.lon).toBe(-70.548);
});

test("admin cannot set invalid base aerodrome", async ({ page }) => {
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  // Try to set invalid ICAO (too short)
  const patchRes1 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SC", lat: -33.456, lon: -70.548 },
  });
  expect(patchRes1.status()).toBe(400);
  const error1 = await patchRes1.json();
  expect(error1.error).toBe("invalid icao");

  // Try to set invalid lat (out of range)
  const patchRes2 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: 95, lon: -70.548 },
  });
  expect(patchRes2.status()).toBe(400);
  const error2 = await patchRes2.json();
  expect(error2.error).toBe("invalid lat");

  // Try to set invalid lon (out of range)
  const patchRes3 = await page.request.patch("/api/admin/club/base", {
    data: { icao: "SCTB", lat: -33.456, lon: -181 },
  });
  expect(patchRes3.status()).toBe(400);
  const error3 = await patchRes3.json();
  expect(error3.error).toBe("invalid lon");
});

test("display base endpoint returns club base", async ({ page }) => {
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  // Create a display session
  const displayRes = await page.request.post("/api/admin/display-sessions", {
    data: { label: "e2e-base-test" },
  });
  expect(displayRes.ok()).toBeTruthy();
  const displayData = await displayRes.json();
  const token = displayData.token;

  // Activate the display session
  const activateRes = await page.request.post("/api/display/activate", {
    data: { token },
  });
  expect(activateRes.ok()).toBeTruthy();

  // Get the base for the display
  const baseRes = await page.request.get("/api/display/base");
  expect(baseRes.ok()).toBeTruthy();
  const baseData = await baseRes.json();
  expect(baseData).toHaveProperty("icao");
  expect(baseData).toHaveProperty("lat");
  expect(baseData).toHaveProperty("lon");
});
