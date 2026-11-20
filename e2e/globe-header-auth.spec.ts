import { test, expect } from "@playwright/test";

test("globe header shows correct auth state when logged out", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeHidden();
  await expect(page.locator('a[href="/club-signup.html"]')).toHaveCount(0);
});

test("globe header shows account link when logged in", async ({ page, request }) => {
  const login = await request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
});
