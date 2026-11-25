import { test, expect } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test("globe library panel opens for a signed-in club session", async ({ page }) => {
  await stubArcgisTiles(page);
  const login = await page.request.post("/api/club/login", {
    data: { email: "pilot@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const authMe = await page.request.get("/api/auth/me");
  expect(authMe.ok()).toBeTruthy();
  const authBody = await authMe.json();
  expect(authBody.authenticated).toBe(true);

  // Disk library list answers JSON (not the SPA HTML, no redirect).
  const flights = await page.request.get("/api/flights");
  expect(flights.ok()).toBeTruthy();
  expect(flights.headers()["content-type"] ?? "").toContain("application/json");
  const flightsBody = await flights.json();
  expect(Array.isArray(flightsBody)).toBe(true);

  await page.goto("/");
  // Auth is resolved during boot before the library panel is interactive.
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  // Boot auto-opens the library for a signed-in pilot, and #btn-library toggles,
  // so wait for the auto-open and only click when the panel stayed closed.
  const panel = page.locator("#library-panel");
  try {
    await panel.waitFor({ state: "visible", timeout: 5_000 });
  } catch {
    await page.click("#btn-library");
  }
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#library-login")).toBeHidden({ timeout: 5_000 });

  const hint = await page.locator("#library-hint").textContent();
  expect(hint).not.toBe("globe.libraryLoginHint");
  expect(hint).not.toContain("Inicia sesión para listar");
  expect(hint).not.toContain("Log in to list");
});

test("library hides remote delete button when canManageLibrary is false", async ({ page }) => {
  await stubArcgisTiles(page);
  const remoteFlights = [
    {
      slug: "vuelo-5",
      name: "Vuelo 5",
      fileName: "vuelo5.gpx",
      updatedAt: "2026-11-24T12:00:00Z",
      createdAt: "2026-11-24T12:00:00Z",
      size: 2048,
    },
  ];
  await page.route("**/api/auth/me", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        authenticated: true,
        configured: true,
        canManageLibrary: false,
        legacyAuthenticated: false,
      }),
    }),
  );
  await page.route("**/api/flights", (route) => {
    if (route.request().method() === "GET") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(remoteFlights),
      });
    }
    return route.continue();
  });

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  // Boot auto-opens the library for a signed-in pilot, and #btn-library toggles,
  // so wait for the auto-open and only click when the panel stayed closed.
  const panel = page.locator("#library-panel");
  try {
    await panel.waitFor({ state: "visible", timeout: 5_000 });
  } catch {
    await page.click("#btn-library");
  }
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#library-list li")).toHaveCount(1, { timeout: 15_000 });
  await expect(page.locator("#library-tools")).toBeHidden();
  await expect(page.locator("#library-list .chip.danger")).toHaveCount(0);
  await expect(page.locator("#library-list .library-item-actions")).toHaveCount(0);
});

test("library delete dialog updates #delete-title and deletes when canManageLibrary is true", async ({
  page,
}) => {
  await stubArcgisTiles(page);
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));

  let remoteFlights = [
    {
      slug: "vuelo-5",
      name: "Vuelo 5",
      fileName: "vuelo5.gpx",
      updatedAt: "2026-11-24T12:00:00Z",
      createdAt: "2026-11-24T12:00:00Z",
      size: 2048,
    },
  ];
  let deletedSlug: string | null = null;

  await page.route("**/api/auth/me", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        authenticated: true,
        configured: true,
        canManageLibrary: true,
        legacyAuthenticated: true,
      }),
    }),
  );
  await page.route("**/api/flights", (route) => {
    if (route.request().method() === "GET") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(remoteFlights),
      });
    }
    return route.continue();
  });
  await page.route("**/api/flights/vuelo-5", (route) => {
    if (route.request().method() === "DELETE") {
      deletedSlug = "vuelo-5";
      remoteFlights = [];
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
    }
    return route.continue();
  });

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  // Boot auto-opens the library for a signed-in pilot, and #btn-library toggles,
  // so wait for the auto-open and only click when the panel stayed closed.
  const panel = page.locator("#library-panel");
  try {
    await panel.waitFor({ state: "visible", timeout: 5_000 });
  } catch {
    await page.click("#btn-library");
  }
  await expect(panel).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#library-list li")).toHaveCount(1, { timeout: 15_000 });
  await expect(page.locator("#library-tools")).toBeVisible();

  await page.locator("#library-list .chip.danger").click();
  await expect(page.locator("#delete-dialog")).toBeVisible();
  const titleText = await page.locator("#delete-title").textContent();
  expect(titleText).toContain("Vuelo 5 (vuelo-5)");
  expect(titleText).not.toContain("{label}");

  await page.locator('#delete-form button[value="ok"]').click();
  await expect.poll(() => deletedSlug).toBe("vuelo-5");
  await expect(page.locator("#library-list li")).toHaveCount(0, { timeout: 15_000 });
  expect(pageErrors).toEqual([]);
});
