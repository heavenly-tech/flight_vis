import { expect, test } from "@playwright/test";

test("unified login shows email form and signup links", async ({ page }) => {
  await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
  await expect(page.locator("#login-form input[name=email]")).toBeVisible();
  await expect(page.getByRole("link", { name: /crea tu cuenta|create account|create your account/i })).toBeVisible();
  await expect(page.getByRole("link", { name: /registrar club|create club/i })).toBeVisible();
});

test("club admin redirect uses unified login", async ({ page }) => {
  await page.goto("/club-admin-types.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/\/login\?/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-admin-types.html");
});

test("password login lands on mis clubs", async ({ page }) => {
  await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
  await page.locator("#login-form input[name=email]").fill("pilot@demo.test");
  await page.locator("#login-form input[name=password]").fill("password12");
  await Promise.all([
    page.waitForURL(/account-clubs\.html/, { timeout: 15_000 }),
    page.locator("#login-form button[type=submit]").click(),
  ]);
});

test("mis clubs: staff slug path for admin next", async ({ page }) => {
  await page.goto("/club-login.html?mode=club&club=demo&next=/club-admin-kiosk-layout.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator("#form input[name=email]").fill("admin@demo.test");
  await page.locator("#form input[name=password]").fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 25_000 }),
    page.locator("#form button[type=submit]").click(),
  ]);
});

test("legacy login route is not linked from public login", async ({ page }) => {
  await page.goto("/login", { waitUntil: "domcontentloaded" });
  await expect(page.locator('a[href*="/login/legacy"]')).toHaveCount(0);
});