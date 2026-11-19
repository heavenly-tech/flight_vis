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
  expect(page.url()).not.toMatch(/password/i);
}

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

test("club login form redirects to pilot profile", async ({ page }) => {
  await loginViaClubForm(page);
  await expect(page).toHaveURL(/club-pilot\.html/);
  const me = await page.request.get("/api/club/i18n/context");
  expect(me.ok()).toBeTruthy();
  const json = (await me.json()) as { userLocale?: string | null };
  expect(json.userLocale).toBeTruthy();
});

test("logged-in pilot locale persists across reload", async ({ page }) => {
  await loginViaClubForm(page);
  const patch = await page.request.patch("/api/club/me/locale", {
    data: { locale: "en" },
  });
  expect(patch.ok()).toBeTruthy();
  await page.reload({ waitUntil: "domcontentloaded" });
  const ctx1 = await page.request.get("/api/club/i18n/context");
  expect((await ctx1.json() as { userLocale?: string }).userLocale).toBe("en");
  await page.reload({ waitUntil: "domcontentloaded" });
  const ctx2 = await page.request.get("/api/club/i18n/context");
  expect((await ctx2.json() as { userLocale?: string }).userLocale).toBe("en");
});
