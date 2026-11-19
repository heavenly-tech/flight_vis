import { test, expect } from "@playwright/test";

test("logged-in pilot locale persists after reload", async ({ page }) => {
  await page.goto("/club-login.html", { waitUntil: "networkidle" });
  await page.fill('input[type="email"]', "admin@demo.test");
  await page.fill('input[type="password"]', "password12");
  await page.locator("button[type=submit]").click();
  await page.waitForURL(/club-pilot\.html/);
  const select = page.locator("#lang-slot select");
  await expect(select).toBeVisible();
  await select.selectOption("en");
  await expect(page.locator("h1")).toContainText(/profile|perfil/i);
  await page.reload({ waitUntil: "networkidle" });
  await expect(page.locator("#lang-slot select")).toHaveValue("en");
  const ctx = await page.request.get("/api/club/i18n/context");
  const json = (await ctx.json()) as { userLocale?: string };
  expect(json.userLocale).toBe("en");
});

test("admin page language switch updates nav", async ({ page }) => {
  await page.goto("/club-login.html", { waitUntil: "networkidle" });
  await page.fill('input[type="email"]', "admin@demo.test");
  await page.fill('input[type="password"]', "password12");
  await page.locator("button[type=submit]").click();
  await page.waitForURL(/club-pilot\.html/);
  await page.goto("/club-admin-pilots.html", { waitUntil: "networkidle" });
  const nav = page.locator("#admin-nav");
  await expect(nav).toBeVisible();
  await page.locator("#lang-slot select").selectOption("en");
  await expect(nav.locator('a[href="/club-admin-types.html"]')).toContainText("Types");
  await page.locator("#lang-slot select").selectOption("es");
  await expect(nav.locator('a[href="/club-admin-types.html"]')).toContainText("Tipos");
});