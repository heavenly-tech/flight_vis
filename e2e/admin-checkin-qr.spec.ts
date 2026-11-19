import { expect, test } from "@playwright/test";

test.describe.configure({ mode: "serial" });

test("admin phone check-in QR is hidden until Mostrar", async ({ page }) => {
  test.setTimeout(90_000);
  await page.goto("/club-login.html?next=/club-admin-checkin-qr.html", {
    waitUntil: "domcontentloaded",
  });
  await page.locator('input[name="email"]').fill("admin@demo.test");
  await page.locator('input[name="password"]').fill("password12");
  await Promise.all([
    page.waitForURL(/\/club-admin-checkin-qr\.html/, { timeout: 30_000 }),
    page.locator('button[type="submit"]').click(),
  ]);
  await expect(page.locator("body")).toHaveAttribute("data-checkin-qr-ready", "1", {
    timeout: 45_000,
  });
  await expect(page.locator("#qr-panel")).toBeHidden();
  const showBtn = page.locator("#btn-show-qr");
  await expect(showBtn).toBeEnabled();
  await Promise.all([
    page.waitForResponse(
      (res) => res.url().includes("/api/admin/checkin-qr.svg") && res.status() === 200,
      { timeout: 15_000 },
    ),
    showBtn.click(),
  ]);
  await expect(page.locator("#qr-img")).toHaveAttribute("src", /\/api\/admin\/checkin-qr\.svg/);
  await expect(page.locator("#qr-panel")).toHaveJSProperty("hidden", false);
  await page.locator("#btn-hide-qr").click();
  await expect(page.locator("#qr-panel")).toBeHidden();
});