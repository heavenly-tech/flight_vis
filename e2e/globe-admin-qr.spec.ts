import { test, expect } from "@playwright/test";
import { installCspObservers, stubArcgisTiles } from "./globe-csp-helpers";

test("admin QR button hidden when logged out", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeHidden();
  
  await expect(page.locator("#btn-admin-qr")).toBeHidden();
  await expect(page.locator("#admin-qr-overlay")).toBeHidden();
  await expect(page.locator("#admin-qr-pill")).toBeHidden();
  
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR button visible for club admin", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Button should be visible next to account button
  await expect(page.locator("#btn-admin-qr")).toBeVisible({ timeout: 15_000 });

  // Overlay and pill should be hidden initially
  await expect(page.locator("#admin-qr-overlay")).toBeHidden();
  await expect(page.locator("#admin-qr-pill")).toBeHidden();
  
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR overlay opens on button click", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Click the button
  await page.locator("#btn-admin-qr").click();

  // Overlay should be visible
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 5_000 });

  // QR image should have the correct src
  await expect(page.locator("#admin-qr-img")).toHaveAttribute("src", /\/api\/admin\/checkin-qr\.svg/, {
    timeout: 5_000,
  });

  // Status should be visible
  await expect(page.locator("#admin-qr-status")).toBeVisible();

  // Pill should still be hidden
  await expect(page.locator("#admin-qr-pill")).toBeHidden();

  // No CSP violations
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR overlay minimizes and restores", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Open overlay
  await page.locator("#btn-admin-qr").click();
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 5_000 });

  // Click minimize
  await page.locator("#admin-qr-minimize").click();

  // Overlay should be hidden, pill should be visible
  await expect(page.locator("#admin-qr-overlay")).toBeHidden({ timeout: 2_000 });
  await expect(page.locator("#admin-qr-pill")).toBeVisible({ timeout: 2_000 });

  // Click pill to restore
  await page.locator("#admin-qr-pill").click();

  // Overlay should be visible again
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 2_000 });
  await expect(page.locator("#admin-qr-pill")).toBeHidden();

  // QR should have fresh src
  await expect(page.locator("#admin-qr-img")).toHaveAttribute("src", /\/api\/admin\/checkin-qr\.svg/, {
    timeout: 5_000,
  });

  // No CSP violations
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR overlay closes on button click", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Open overlay
  await page.locator("#btn-admin-qr").click();
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 5_000 });

  // Click button again to close
  await page.locator("#btn-admin-qr").click();

  // Overlay should be hidden
  await expect(page.locator("#admin-qr-overlay")).toBeHidden({ timeout: 2_000 });
  await expect(page.locator("#admin-qr-pill")).toBeHidden();

  // No CSP violations
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR button hidden for non-admin logged-in pilot", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as a regular pilot (not admin)
  const login = await page.request.post("/api/club/login", {
    data: { email: "pilot@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Button should be hidden for non-admin
  await expect(page.locator("#btn-admin-qr")).toBeHidden();
  await expect(page.locator("#admin-qr-overlay")).toBeHidden();
  await expect(page.locator("#admin-qr-pill")).toBeHidden();

  // No CSP violations
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test("admin QR overlay stays within viewport on drag and resize", async ({ page }) => {
  await stubArcgisTiles(page);
  const { consoleErrors, pageErrors } = installCspObservers(page);
  
  // Log in as admin
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  
  await page.goto("/");
  await expect(page.locator("#btn-login")).toBeHidden({ timeout: 15_000 });
  await expect(page.locator("#btn-account")).toBeVisible({ timeout: 15_000 });
  
  // Open overlay
  await page.locator("#btn-admin-qr").click();
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 5_000 });
  
  const overlay = page.locator("#admin-qr-overlay");
  const header = page.locator(".admin-qr-header");
  const resizeHandle = page.locator("#admin-qr-resize");
  
  const vw = page.viewportSize!.width;
  const vh = page.viewportSize!.height;
  
  // Step 1: Drag top-left - grab header at left side, move to (1, 1)
  const headerBox1 = await header.boundingBox();
  expect(headerBox1).toBeDefined();
  const grabX1 = headerBox1!.x + 20;
  const grabY1 = headerBox1!.y + headerBox1!.height / 2;
  
  await page.mouse.move(grabX1, grabY1);
  await page.mouse.down();
  await page.mouse.move(1, 1, { steps: 10 });
  await page.mouse.up();
  
  const afterDrag1 = await overlay.boundingBox();
  expect(afterDrag1!.x).toBeGreaterThanOrEqual(0);
  expect(afterDrag1!.y).toBeGreaterThanOrEqual(0);
  
  // Step 2: Drag bottom-right - re-read header box, grab same relative point, move to (vw-1, vh-1)
  const headerBox2 = await header.boundingBox();
  expect(headerBox2).toBeDefined();
  const grabX2 = headerBox2!.x + 20;
  const grabY2 = headerBox2!.y + headerBox2!.height / 2;
  
  await page.mouse.move(grabX2, grabY2);
  await page.mouse.down();
  await page.mouse.move(vw - 1, vh - 1, { steps: 10 });
  await page.mouse.up();
  
  const afterDrag2 = await overlay.boundingBox();
  expect(afterDrag2!.x + afterDrag2!.width).toBeLessThanOrEqual(vw + 1);
  expect(afterDrag2!.y + afterDrag2!.height).toBeLessThanOrEqual(vh + 1);
  
  // Step 3: Drag back to top-left, then drag resize handle to (vw-1, vh-1)
  const headerBox3 = await header.boundingBox();
  expect(headerBox3).toBeDefined();
  const grabX3 = headerBox3!.x + 20;
  const grabY3 = headerBox3!.y + headerBox3!.height / 2;
  
  await page.mouse.move(grabX3, grabY3);
  await page.mouse.down();
  await page.mouse.move(1, 1, { steps: 10 });
  await page.mouse.up();
  
  const resizeBox = await resizeHandle.boundingBox();
  expect(resizeBox).toBeDefined();
  
  await page.mouse.move(resizeBox!.x + resizeBox!.width / 2, resizeBox!.y + resizeBox!.height / 2);
  await page.mouse.down();
  await page.mouse.move(vw - 1, vh - 1, { steps: 10 });
  await page.mouse.up();
  
  const afterResize = await overlay.boundingBox();
  expect(afterResize!.width).toBeLessThanOrEqual(vw + 1);
  expect(afterResize!.height).toBeLessThanOrEqual(vh + 1);
  expect(afterResize!.x + afterResize!.width).toBeLessThanOrEqual(vw + 1);
  expect(afterResize!.y + afterResize!.height).toBeLessThanOrEqual(vh + 1);
  
  // Test: minimize button click works (not swallowed by drag)
  await page.locator("#admin-qr-minimize").click();
  await expect(page.locator("#admin-qr-overlay")).toBeHidden({ timeout: 2_000 });
  await expect(page.locator("#admin-qr-pill")).toBeVisible({ timeout: 2_000 });
  
  // Restore
  await page.locator("#admin-qr-pill").click();
  await expect(page.locator("#admin-qr-overlay")).toBeVisible({ timeout: 2_000 });
  
  // No CSP violations
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});
