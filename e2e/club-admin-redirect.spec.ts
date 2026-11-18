import { expect, test } from "@playwright/test";

test("unauthenticated club admin redirects to club login with next", async ({ page }) => {
  await Promise.all([
    page.waitForURL(/\/club-login\.html/, { timeout: 20_000 }),
    page.goto("/club-admin-types.html", { waitUntil: "commit" }),
  ]);
  const url = new URL(page.url());
  expect(url.pathname).toBe("/club-login.html");
  expect(url.searchParams.get("next")).toBe("/club-admin-types.html");
});

test("club login honours safe next when session already exists", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 20_000 }),
    page.goto("/club-login.html?next=/club-admin-kiosk-layout.html", {
      waitUntil: "commit",
    }),
  ]);
});

test("club login form honours safe next after sign-in", async ({ page }) => {
  await page.goto("/club-login.html?next=/club-admin-kiosk-layout.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 20_000 }),
    page.locator("button[type=submit]").click(),
  ]);
});