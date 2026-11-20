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
  
  // Get initial position and size
  const initialBox = await overlay.boundingBox();
  expect(initialBox).toBeDefined();
  
  // Test: drag header far beyond top-left (to -500, -500)
  // We need to click on the header but not on the minimize button
  // The minimize button is typically on the right side of the header
  const headerBox = await header.boundingBox();
  expect(headerBox).toBeDefined();
  
  // Click at the left side of the header (away from minimize button which is on the right)
  const clickX = headerBox!.x + 20;
  const clickY = headerBox!.y + headerBox!.height / 2;
  
  await page.mouse.move(clickX, clickY);
  await page.mouse.down();
  
  // Drag far beyond top-left
  await page.mouse.move(-500, -500);
  await page.mouse.up();
  
  // Assert overlay stays within viewport (x, y >= 0)
  const afterDragBox = await overlay.boundingBox();
  expect(afterDragBox!.x).toBeGreaterThanOrEqual(0);
  expect(afterDragBox!.y).toBeGreaterThanOrEqual(0);
  
  // Test: drag far beyond bottom-right
  await page.mouse.move(clickX, clickY);
  await page.mouse.down();
  await page.mouse.move(page.viewportSize!.width + 500, page.viewportSize!.height + 500);
  await page.mouse.up();
  
  // Assert overlay stays within viewport (right/bottom edges <= viewport)
  const afterDragBox2 = await overlay.boundingBox();
  expect(afterDragBox2!.x + afterDragBox2!.width).toBeLessThanOrEqual(page.viewportSize!.width);
  expect(afterDragBox2!.y + afterDragBox2!.height).toBeLessThanOrEqual(page.viewportSize!.height);
  
  // Test: drag resize handle by +5000,+5000
  const resizeBox = await resizeHandle.boundingBox();
  expect(resizeBox).toBeDefined();
  
  await page.mouse.move(resizeBox!.x + resizeBox!.width / 2, resizeBox!.y + resizeBox!.height / 2);
  await page.mouse.down();
  await page.mouse.move(resizeBox!.x + resizeBox!.width / 2 + 5000, resizeBox!.y + resizeBox!.height / 2 + 5000);
  await page.mouse.up();
  
  // Assert overlay width/height <= viewport
  const afterResizeBox = await overlay.boundingBox();
  expect(afterResizeBox!.width).toBeLessThanOrEqual(page.viewportSize!.width);
  expect(afterResizeBox!.height).toBeLessThanOrEqual(page.viewportSize!.height);
  
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
