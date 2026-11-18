import { expect, test } from "@playwright/test";

test("login chooser links to club and pilot paths", async ({ page }) => {
  await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
  await expect(page.getByRole("heading", { name: /iniciar sesión|sign in/i })).toBeVisible();
  const club = page.getByRole("link", { name: /iniciar sesión como club|sign in as club/i });
  const pilot = page.getByRole("link", { name: /iniciar sesión como piloto|sign in as pilot/i });
  await expect(club).toHaveAttribute("href", /mode=club/);
  await expect(pilot).toHaveAttribute("href", /mode=pilot/);
});

test("club admin redirect uses club login mode", async ({ page }) => {
  await page.goto("/club-admin-types.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/club-login\.html.*mode=club/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-admin-types.html");
});

test("club path: slug sign-in lands on admin next", async ({ page }) => {
  await page.goto("/club-login.html?mode=club&club=demo&next=/club-admin-kiosk-layout.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 25_000 }),
    page.locator("#form button[type=submit]").click(),
  ]);
});

test("club path: non-admin membership denied", async ({ page }) => {
  const email = `pilot-${Date.now()}@demo.test`;
  const reg = await page.request.post("/api/club/register", {
    data: {
      email,
      password: "password12",
      name: "E2E Pilot",
      licenseNo: "E2E-1",
      slug: "demo",
    },
  });
  expect(reg.ok()).toBeTruthy();
  await page.goto("/club-login.html?mode=club&club=demo&next=/club-admin-map.html");
  await page.locator('input[name="email"]').fill(email);
  await page.locator('input[name="password"]').fill("password12");
  await page.locator("#form button[type=submit]").click();
  await expect(page.locator("#msg")).toContainText(/administrador|staff|admin or staff/i);
  await expect(page).not.toHaveURL(/club-admin-map/);
});

test("pilot path: sign-in shows my clubs and create account link", async ({ page }) => {
  await page.goto("/club-login.html?mode=pilot", { waitUntil: "domcontentloaded" });
  await expect(page.getByRole("link", { name: /crea tu cuenta|create your account/i })).toBeVisible();
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  await page.locator("#form button[type=submit]").click();
  await expect(page.getByRole("heading", { name: /mis clubs|my clubs/i })).toBeVisible({ timeout: 15_000 });
});