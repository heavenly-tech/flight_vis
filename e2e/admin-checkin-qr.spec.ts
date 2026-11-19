import { expect, test } from "@playwright/test";

test.describe.configure({ mode: "serial" });

test("admin phone check-in QR is hidden until Mostrar", async ({ page }) => {
  test.setTimeout(90_000);
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/club-admin-checkin-qr.html", { waitUntil: "load" });
  await expect(page).not.toHaveURL(/\/club-login\.html/);
  const meInPage = await page.evaluate(async () => {
    const res = await fetch("/api/club/me", { credentials: "same-origin" });
    return res.ok;
  });
  expect(meInPage).toBeTruthy();
  await expect.poll(
    async () =>
      page.evaluate(() => (window as unknown as { __fvClubCheckinQrBoot?: string }).__fvClubCheckinQrBoot),
    { timeout: 60_000 },
  ).toBe("ready");
  await expect(page.locator("#btn-show-qr")).toBeEnabled();
  await expect(page.locator("#qr-panel")).toBeHidden();
  await page.locator("#btn-show-qr").click();
  await expect(page.locator("#qr-panel")).toHaveJSProperty("hidden", false);
  await expect(page.locator("#qr-img")).toHaveAttribute("src", /\/api\/admin\/checkin-qr\.svg/, {
    timeout: 15_000,
  });
  await page.locator("#btn-hide-qr").click();
  await expect(page.locator("#qr-panel")).toBeHidden();
});