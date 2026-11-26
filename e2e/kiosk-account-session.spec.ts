import { expect, test, type Page } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.setTimeout(60_000);

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

async function signInAsDemoAdminAccount(page: Page): Promise<void> {
  const login = await page.request.post("/api/account/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok(), "POST /api/account/login").toBeTruthy();

  const meRes = await page.request.get("/api/account/me");
  expect(meRes.ok(), "GET /api/account/me").toBeTruthy();
  const me = (await meRes.json()) as {
    memberships?: Array<{ id: string; role?: string; status: string }>;
  };
  const adminMem = (me.memberships ?? []).find(
    (m) => m.status === "active" && m.role === "admin",
  );
  expect(adminMem, "active demo admin membership").toBeTruthy();

  const sw = await page.request.post("/api/account/switch-club", {
    data: { membershipId: adminMem!.id },
  });
  expect(sw.ok(), "POST /api/account/switch-club").toBeTruthy();
}

test.describe("kiosk account session and two-window admin sync", () => {
  test("signed-out user is redirected to login with safe next from kiosk routes", async ({
    page,
  }) => {
    await page.goto("/club-screen.html", { waitUntil: "load" });
    await expect(page).toHaveURL(/\/login\?/);
    expect(new URL(page.url()).searchParams.get("next")).toBe("/club-screen.html");

    await page.goto("/club/screen", { waitUntil: "load" });
    await expect(page).toHaveURL(/\/login\?/);
    expect(new URL(page.url()).searchParams.get("next")).toBe("/club-screen.html");

    await page.goto("/c/demo/screen", { waitUntil: "load" });
    await expect(page).toHaveURL(/\/login\?/);
    expect(new URL(page.url()).searchParams.get("next")).toBe("/c/demo/screen");
  });

  test("signed-in admin sees kiosk directly without activation form and opens it from Abrir pantalla del club link", async ({
    page,
  }) => {
    await signInAsDemoAdminAccount(page);
    await stubArcgisTiles(page);

    await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });
    const openLink = page.locator("a[data-open-kiosk-link]");
    await expect(openLink).toBeVisible();
    await expect(openLink).toHaveText("Abrir pantalla del club");
    await expect(openLink).toHaveAttribute("target", "_blank");
    await expect(openLink).toHaveAttribute("rel", "noopener");
    await expect(openLink).toHaveAttribute("href", "/c/demo/screen");

    const popupPromise = page.waitForEvent("popup");
    await openLink.click();
    const screenPage = await popupPromise;
    await stubArcgisTiles(screenPage);
    await screenPage.waitForLoadState("networkidle");

    await expect(screenPage).toHaveURL(/\/c\/demo\/screen$/);
    await expect(screenPage.locator("#activate")).toHaveCount(0);
    await expect(screenPage.locator("#activate-token")).toHaveCount(0);
    await expect(screenPage.locator('[data-testid="kiosk-map"]')).toBeVisible({ timeout: 15_000 });
    await expect(screenPage.locator("#btn-local-qr")).toBeVisible({ timeout: 10_000 });
    await screenPage.close();
  });

  test("two pages in one browser context stay authenticated, sync admin changes immediately via channel, and log out cleanly with no loop", async ({
    context,
  }) => {
    const adminPage = await context.newPage();
    await stubArcgisTiles(adminPage);
    await signInAsDemoAdminAccount(adminPage);

    const screenPage = await context.newPage();
    await stubArcgisTiles(screenPage);

    const screenErrors: string[] = [];
    screenPage.on("pageerror", (err) => screenErrors.push(String(err)));

    try {
      // 1. Open admin page and kiosk screen in two pages sharing one browser context.
      await adminPage.goto("/club-admin-kiosk-layout.html", { waitUntil: "networkidle" });
      await screenPage.goto("/club-screen.html", { waitUntil: "networkidle" });

      await expect(screenPage.locator("#activate")).toHaveCount(0);
      await expect(screenPage.locator('[data-testid="kiosk-map"]')).toBeVisible({ timeout: 15_000 });
      await expect(screenPage.locator("#btn-local-qr")).toBeVisible({ timeout: 10_000 });

      // Both stay authenticated across multiple navigations and reloads.
      await adminPage.reload({ waitUntil: "networkidle" });
      await expect(adminPage).toHaveURL(/\/club-admin-kiosk-layout\.html/);
      await adminPage.goto("/club-admin-map.html", { waitUntil: "networkidle" });
      await expect(adminPage).toHaveURL(/\/club-admin-map\.html/);
      await adminPage.goto("/club-admin-kiosk-layout.html", { waitUntil: "networkidle" });
      await expect(adminPage).toHaveURL(/\/club-admin-kiosk-layout\.html/);

      await screenPage.reload({ waitUntil: "networkidle" });
      await expect(screenPage).toHaveURL(/\/club-screen\.html/);
      await expect(screenPage.locator('[data-testid="kiosk-map"]')).toBeVisible({ timeout: 15_000 });

      // 2. Admin saves a layout change -> screen refetches promptly (< 2s) via BroadcastChannel/storage.
      const visibleCheckbox = adminPage.locator("#checkin-qr-visible");
      await expect(visibleCheckbox).toBeVisible();
      const initiallyChecked = await visibleCheckbox.isChecked();
      const toggledTarget = !initiallyChecked;

      const layoutRefetchPromise = screenPage.waitForRequest(
        (req) => req.url().includes("/api/display/kiosk-layout") && !req.url().includes("/stream"),
        { timeout: 2_500 },
      );
      if (toggledTarget) {
        await visibleCheckbox.check();
      } else {
        await visibleCheckbox.uncheck();
      }
      await adminPage.locator("#btn-save").click();
      await expect(adminPage.locator("#save-status")).toHaveText("Guardado", { timeout: 10_000 });
      await layoutRefetchPromise;

      if (toggledTarget) {
        await expect(screenPage.locator("#qr-wrap")).toBeVisible({ timeout: 5_000 });
      } else {
        await expect(screenPage.locator("#qr-wrap")).toBeHidden({ timeout: 5_000 });
      }

      // Restore visibility back to checked so demo club stays in default state.
      const restoreRefetchPromise = screenPage.waitForRequest(
        (req) => req.url().includes("/api/display/kiosk-layout") && !req.url().includes("/stream"),
        { timeout: 2_500 },
      );
      await visibleCheckbox.check();
      await adminPage.locator("#btn-save").click();
      await expect(adminPage.locator("#save-status")).toHaveText("Guardado", { timeout: 10_000 });
      await restoreRefetchPromise;
      await expect(screenPage.locator("#qr-wrap")).toBeVisible({ timeout: 5_000 });

      // Also verify tail filter save on admin map triggers prompt /api/display/map refetch on screen.
      await adminPage.goto("/club-admin-map.html", { waitUntil: "networkidle" });
      const mapRefetchPromise = screenPage.waitForRequest(
        (req) => req.url().includes("/api/display/map"),
        { timeout: 2_500 },
      );
      await adminPage.locator("#save").click();
      await expect(adminPage.locator("#tails-msg")).toHaveText("Selección guardada.", {
        timeout: 10_000,
      });
      await mapRefetchPromise;

      // 3. Logout in the admin window cleanly redirects the screen to /login?next=/club-screen.html with no error loop.
      let loginNavigations = 0;
      screenPage.on("framenavigated", (frame) => {
        if (frame === screenPage.mainFrame() && frame.url().includes("/login")) {
          loginNavigations += 1;
        }
      });

      const logoutBtn = adminPage.locator("#admin-header-logout");
      await expect(logoutBtn).toBeVisible();
      await logoutBtn.click();

      await expect(adminPage).toHaveURL(/\/login/, { timeout: 10_000 });
      await expect(screenPage).toHaveURL(/\/login\?next=%2Fclub-screen\.html/, {
        timeout: 10_000,
      });

      // Wait briefly to confirm no redirect/error loop occurs after reaching /login.
      await screenPage.waitForTimeout(800);
      expect(loginNavigations).toBe(1);
      expect(screenErrors).toEqual([]);
    } finally {
      await screenPage.close().catch(() => {});
      await adminPage.close().catch(() => {});
    }
  });
});
