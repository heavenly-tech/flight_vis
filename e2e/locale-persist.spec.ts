import { test, expect } from "@playwright/test";

async function loginViaClubForm(page: import("@playwright/test").Page): Promise<void> {
  await page.goto("/club-login.html", { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.__fvClubLoginReady === true);
  await page.fill('input[name="email"]', "admin@demo.test");
  await page.fill('input[name="password"]', "password12");
  await Promise.all([
    page.waitForURL(/club-pilot\.html/),
    page.locator("#form button[type=submit]").click(),
  ]);
}

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

test("club login form redirects to pilot profile", async ({ page }) => {
  await loginViaClubForm(page);
  await expect(page).toHaveURL(/club-pilot\.html/);
  await expect(page.locator("h1")).toContainText(/profile|perfil/i);
});

test("logged-in pilot locale persists after reload", async ({ page }) => {
  await loginViaClubForm(page);
  const select = page.locator("#lang-slot select");
  await expect(select).toBeVisible({ timeout: 60_000 });
  await Promise.all([page.waitForNavigation(), select.selectOption("en")]);
  await expect(select).toHaveValue("en");
  await expect(page.locator("h1")).toContainText(/profile/i);
  await page.reload({ waitUntil: "domcontentloaded" });
  await expect(select).toHaveValue("en");
  const ctx = await page.request.get("/api/club/i18n/context");
  const json = (await ctx.json()) as { userLocale?: string };
  expect(json.userLocale).toBe("en");
});

test("admin page language switch updates nav", async ({ page }) => {
  await loginViaClubForm(page);
  await page.goto("/club-admin-pilots.html", { waitUntil: "domcontentloaded" });
  const nav = page.locator("#admin-nav a");
  await expect(nav.first()).toBeVisible();
  const select = page.locator("#lang-slot select");
  await Promise.all([page.waitForNavigation(), select.selectOption("en")]);
  await expect(nav.filter({ hasText: "Types" }).first()).toBeVisible();
  await Promise.all([page.waitForNavigation(), select.selectOption("es")]);
  await expect(nav.filter({ hasText: "Tipos" }).first()).toBeVisible();
});