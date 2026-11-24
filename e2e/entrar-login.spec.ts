import { expect, test } from "@playwright/test";

test("unified login keeps magic link default and puts pilot signup primary over club signup", async ({
  page,
}) => {
  await page.goto("/login?next=/club-pilot.html", { waitUntil: "load" });
  await expect(page.locator("#email")).toBeVisible();
  await expect(page.locator("#submit-magic")).toBeVisible();
  await expect(page.locator("#password-reveal")).toBeHidden();
  await expect(page.locator("#submit-password")).toBeHidden();
  await expect(page.locator("#login-form")).toHaveAttribute(
    "action",
    "/api/account/magic-link",
  );

  const pilotLink = page.locator('a#register-pilot[href="/club-register.html"]');
  const clubLink = page.locator('a#register-club[href="/club-signup.html"]');
  await expect(pilotLink).toBeVisible();
  await expect(clubLink).toBeVisible();
  await expect(pilotLink).toHaveClass(/\bprimary\b/);
  await expect(clubLink).toHaveClass(/\bsecondary\b/);
  await expect(page.locator('a[href="/signup.html"], a[href="/signup"]')).toHaveCount(0);

  const order = await page
    .locator("#register-pilot, #register-club")
    .evaluateAll((nodes) => nodes.map((n) => n.id));
  expect(order).toEqual(["register-pilot", "register-club"]);
});

test("club admin redirect uses unified login", async ({ page }) => {
  await page.goto("/club-admin-types.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/\/login\?/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-admin-types.html");
});

test("account password login reaches mis clubs", async ({ page }) => {
  const login = await page.request.post("/api/account/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/account-clubs.html?next=/club-pilot.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/account-clubs\.html/);
  await expect(page.locator("#pick-panel")).toBeVisible();
});

test("password mode account login reaches admin next", async ({ page }) => {
  await page.goto("/login?mode=password&next=/club-admin-kiosk-layout.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator("#login-form input[name=email]").fill("admin@demo.test");
  await page.locator("#login-form input[name=password]").fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 25_000 }),
    page.locator("#submit-password").click(),
  ]);
});

test("legacy login route is not linked from public login", async ({ page }) => {
  await page.goto("/login", { waitUntil: "domcontentloaded" });
  await expect(page.locator('a[href*="/login/legacy"]')).toHaveCount(0);
  await expect(page.locator('a[href*="club-login"]')).toHaveCount(0);
});