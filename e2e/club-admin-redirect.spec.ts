import { expect, test } from "@playwright/test";

test("unauthenticated club admin redirects to club login with next", async ({ page }) => {
  await page.goto("/club-admin-types.html", { waitUntil: "domcontentloaded" });
  await page.waitForURL(/\/club-login\.html/, { timeout: 15_000 });
  const url = new URL(page.url());
  expect(url.pathname).toBe("/club-login.html");
  expect(url.searchParams.get("next")).toBe("/club-admin-types.html");
});

test("club login honours safe next after sign-in", async ({ page }) => {
  await page.goto("/club-login.html?next=/club-admin-kiosk-layout.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  const loginOk = page.waitForResponse(
    (res) => res.url().includes("/api/club/login") && res.status() === 200,
  );
  await Promise.all([
    loginOk,
    page.locator("button[type=submit]").click(),
  ]);
  await page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 20_000 });
});