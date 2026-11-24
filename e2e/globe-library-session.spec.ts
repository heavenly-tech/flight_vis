import { test, expect } from "@playwright/test";

test("globe library panel opens for a signed-in club session", async ({ page }) => {
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
