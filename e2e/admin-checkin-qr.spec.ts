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
  await expect(page.locator("#btn-show-qr")).toBeAttached({ timeout: 30_000 });
  await page.waitForFunction(
    () => {
      const b = document.querySelector("#btn-show-qr") as HTMLButtonElement | null;
      return b !== null && !b.disabled;
    },
    { timeout: 60_000 },
  );
  await expect(page.locator("#qr-panel")).toBeHidden();
  await page.locator("#btn-show-qr").evaluate((el) => {
    (el as HTMLButtonElement).click();
  });
  await expect(page.locator("#qr-panel")).toHaveJSProperty("hidden", false, { timeout: 10_000 });
  await expect(page.locator("#qr-img")).toHaveAttribute("src", /\/api\/admin\/checkin-qr\.svg/, {
    timeout: 15_000,
  });
  await page.locator("#btn-hide-qr").click();
  await expect(page.locator("#qr-panel")).toBeHidden();
});