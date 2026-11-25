import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.setTimeout(60_000);
test.use({ locale: "es-CL" });

test("public live map with zero flights renders club name, empty state, and base map", async ({
  page,
}) => {
  await stubArcgisTiles(page);

  // Use the real backend response for clubName & base aerodrome, and clear seeded demo flights
  // so we deterministically verify the zero-flights state.
  let backendClubName: unknown = null;
  let backendBase: unknown = null;
  await page.route("**/api/club/map/public*", async (route) => {
    const res = await route.fetch();
    const json = (await res.json()) as Record<string, unknown>;
    backendClubName = json.clubName;
    backendBase = json.base;
    await route.fulfill({
      status: res.status(),
      contentType: "application/json",
      body: JSON.stringify({ ...json, aircraft: [] }),
    });
  });

  await page.goto("/public-map.html?club=demo");

  const clubName = page.locator("#club-name");
  await expect(clubName).toBeVisible({ timeout: 15_000 });
  await expect(clubName).not.toHaveText(/Cargando|Loading|^\s*$/);
  expect(typeof backendClubName).toBe("string");
  expect((backendClubName as string).length).toBeGreaterThan(0);
  expect(backendBase).toBeTruthy();
  await expect(clubName).toHaveText(backendClubName as string);

  const map = page.locator('[data-testid="public-map"]');
  await expect(map).toBeVisible({ timeout: 15_000 });
  const mapBox = await map.boundingBox();
  expect(mapBox).not.toBeNull();
  expect(mapBox!.width).toBeGreaterThan(200);
  expect(mapBox!.height).toBeGreaterThan(200);

  await expect
    .poll(() => map.locator("img[data-tile]").count(), { timeout: 15_000 })
    .toBeGreaterThan(0);
  await expect(map).toHaveAttribute("data-view", /^(base|fallback)$/);

  const emptyState = page.locator("#empty-state");
  await expect(emptyState).toBeVisible({ timeout: 15_000 });
  await expect(emptyState).toHaveText("No hay vuelos públicos activos en este momento.");
  await expect(page.locator("#list li")).toHaveCount(0);
});

test("public live map renders fallback view without base and fits active public flights", async ({
  page,
}) => {
  await stubArcgisTiles(page);

  await page.route("**/api/club/map/public*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        club: "sin-base",
        clubName: "Club Sin Base",
        base: { icao: null, lat: null, lon: null },
        aircraft: [],
      }),
    });
  });

  await page.goto("/public-map.html?club=sin-base");

  await expect(page.locator("#club-name")).toHaveText("Club Sin Base", { timeout: 15_000 });
  const map = page.locator('[data-testid="public-map"]');
  await expect(map).toHaveAttribute("data-view", "fallback", { timeout: 15_000 });
  await expect
    .poll(() => map.locator("img[data-tile]").count(), { timeout: 15_000 })
    .toBeGreaterThan(0);
  await expect(page.locator("#empty-state")).toBeVisible();

  await page.unroute("**/api/club/map/public*");
  await page.route("**/api/club/map/public*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        club: "cas-test",
        clubName: "Club Aéreo de Santiago",
        base: { icao: "SCTB", lat: -33.456, lon: -70.548 },
        aircraft: [
          {
            tail: "CC-ABC",
            pilotName: "Piloto Prueba",
            fix: { lat: -33.48, lon: -70.58, alt: 1200 },
          },
        ],
      }),
    });
  });

  await page.goto("/public-map.html?club=cas-test");
  await expect(page.locator("#club-name")).toHaveText("Club Aéreo de Santiago", {
    timeout: 15_000,
  });
  await expect(map).toHaveAttribute("data-view", "flights", { timeout: 15_000 });
  await expect(map.locator('[data-marker="base"]')).toHaveText("SCTB");
  await expect(map.locator('[data-marker="aircraft"][data-tail="CC-ABC"]')).toBeVisible();
  await expect(page.locator("#empty-state")).toBeHidden();
  await expect(page.locator("#list li")).toHaveText(["CC-ABC · Piloto Prueba"]);
});
