import { expect, test } from "@playwright/test";

test.describe.configure({ mode: "serial" });

test("admin phone check-in QR is hidden until Mostrar", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/club-admin-checkin-qr.html", { waitUntil: "networkidle" });
  await expect(page).not.toHaveURL(/\/club-login\.html/);
  await expect(page.locator("body")).toHaveAttribute("data-checkin-qr-ready", "1", {
    timeout: 30_000,
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