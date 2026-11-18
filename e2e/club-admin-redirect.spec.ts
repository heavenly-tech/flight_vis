import { expect, test } from "@playwright/test";

const adminPagesRedirectToClubLogin = [
  "/club-admin-types.html",
  "/club-admin-matrix.html",
  "/club-admin-import.html",
  "/club-admin-audit.html",
] as const;

for (const adminPath of adminPagesRedirectToClubLogin) {
  test(`unauthenticated ${adminPath} redirects to club login with next`, async ({ page }) => {
    await page.goto(adminPath, { waitUntil: "load" });
    await expect(page).toHaveURL(/\/club-login\.html/);
    expect(new URL(page.url()).searchParams.get("next")).toBe(adminPath);
  });
}

test("club login honours safe next when session already exists", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 20_000 }),
    page.goto("/club-login.html?next=/club-admin-kiosk-layout.html", {
      waitUntil: "load",
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