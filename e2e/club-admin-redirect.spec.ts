import { expect, test } from "@playwright/test";

test("unauthenticated club admin redirects to club login with next", async ({ page }) => {
  await Promise.all([
    page.waitForURL(/\/club-login\.html\?next=%2Fclub-admin-types\.html/, { timeout: 15_000 }),
    page.goto("/club-admin-types.html"),
  ]);
});

test("club login honours safe next after sign-in", async ({ page }) => {
  await page.goto("/club-login.html?next=%2Fclub-admin-kiosk-layout.html", {
    waitUntil: "networkidle",
  });
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  await page.locator("button[type=submit]").click();
  await page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 15_000 });
});