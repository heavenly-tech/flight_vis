import { test, expect } from "@playwright/test";

async function loginDemoAdmin(page: import("@playwright/test").Page): Promise<void> {
  const res = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(res.ok()).toBeTruthy();
}

test("logged-in pilot locale persists after reload", async ({ page }) => {
  await loginDemoAdmin(page);
  await page.goto("/club-pilot.html", { waitUntil: "networkidle" });
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
  await loginDemoAdmin(page);
  await page.goto("/club-admin-pilots.html", { waitUntil: "networkidle" });
  const nav = page.locator("#admin-nav");
  await expect(nav).toBeVisible();
  await page.locator("#lang-slot select").selectOption("en");
  await expect(nav.locator('a[href="/club-admin-types.html"]')).toContainText("Types");
  await page.locator("#lang-slot select").selectOption("es");
  await expect(nav.locator('a[href="/club-admin-types.html"]')).toContainText("Tipos");
});