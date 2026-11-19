import { expect, test } from "@playwright/test";

test("kiosk layout math stays inside preview box", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
  await page.goto("/club-admin-kiosk-layout.html", { waitUntil: "networkidle" });
  const frame = page.locator("#preview-frame");
  await expect(frame).toBeVisible();
  await expect(page.locator("#preview-qr img")).toBeVisible();
  const box = await frame.boundingBox();
  expect(box).not.toBeNull();
  const qr = page.locator("#preview-qr img");
  const qrBox = await qr.boundingBox();
  expect(qrBox).not.toBeNull();
  if (box && qrBox) {
    expect(qrBox.x).toBeGreaterThanOrEqual(box.x - 1);
    expect(qrBox.y).toBeGreaterThanOrEqual(box.y - 1);
    expect(qrBox.x + qrBox.width).toBeLessThanOrEqual(box.x + box.width + 1);
    expect(qrBox.y + qrBox.height).toBeLessThanOrEqual(box.y + box.height + 1);
  }
});