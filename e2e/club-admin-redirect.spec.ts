import { expect, test } from "@playwright/test";

test("unauthenticated club admin redirects to club login with next", async ({ page }) => {
  await page.goto("/club-admin-types.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/\/club-login\.html.*mode=club/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-admin-types.html");
});

test("club login honours safe next when session already exists", async ({ page }) => {
  const login = await page.request.post("/api/account/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  const me = await page.request.get("/api/account/me");
  expect(me.ok()).toBeTruthy();
  const body = await me.json();
  const adminMem = (body.memberships as Array<{ id: string; role?: string; status: string }>).find(
    (m) => m.status === "active" && m.role === "admin",
  );
  expect(adminMem).toBeTruthy();
  const sw = await page.request.post("/api/account/switch-club", {
    data: { membershipId: adminMem!.id },
  });
  expect(sw.ok()).toBeTruthy();
  await page.goto("/club-login.html?mode=club&club=demo&next=/club-admin-kiosk-layout.html", {
    waitUntil: "load",
  });
  await page.waitForURL(/\/club-admin-kiosk-layout\.html/, { timeout: 20_000 });
});

test("club login form honours safe next after sign-in", async ({ page }) => {
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