import { expect, test } from "@playwright/test";

test("login chooser links to club and pilot paths", async ({ page }) => {
  await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
  const club = page.locator("#btn-club");
  const pilot = page.locator("#btn-pilot");
  await expect(club).toBeVisible();
  await expect(pilot).toBeVisible();
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
  await page.locator("#form input[name=email]").fill("admin@demo.test");
  await page.locator("#form input[name=password]").fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 25_000 }),
    page.locator("#form button[type=submit]").click(),
  ]);
});

test("club path: non-admin membership denied", async ({ page }) => {
  const email = `pilot-${Date.now()}@demo.test`;
  expect(
    (await page.request.post("/api/account/signup", { data: { email, password: "password12" } })).ok(),
  ).toBeTruthy();
  expect(
    (await page.request.post("/api/account/login", { data: { email, password: "password12" } })).ok(),
  ).toBeTruthy();
  expect(
    (
      await page.request.post("/api/account/join", {
        data: { slug: "demo", name: "E2E Pilot", license_no: "E2E-1" },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto("/club-login.html?mode=club&club=demo&next=/club-admin-map.html");
  await page.locator("#form input[name=email]").fill(email);
  await page.locator("#form input[name=password]").fill("password12");
  await page.locator("#form button[type=submit]").click();
  await expect(page.locator("#msg")).toContainText(/administrador|staff|admin or staff/i);
  await expect(page).not.toHaveURL(/club-admin-map/);
});

test("pilot path: sign-in shows my clubs and create account link", async ({ page }) => {
  await page.goto("/club-login.html?mode=pilot&step=login", { waitUntil: "domcontentloaded" });
  await expect(page.getByRole("link", { name: /crea tu cuenta|create your account/i })).toBeVisible();
  await page.locator("#form input[name=email]").fill("pilot@demo.test");
  await page.locator("#form input[name=password]").fill("password12");
  await page.locator("#form button[type=submit]").click();
  await page.waitForURL(/\/club-pilot\.html/, { timeout: 15_000 });
});