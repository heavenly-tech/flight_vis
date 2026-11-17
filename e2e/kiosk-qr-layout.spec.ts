import { expect, test } from "@playwright/test";

test("kiosk layout math stays inside preview box", async ({ page }) => {
  await page.goto("/club-admin-kiosk-layout.html");
  const frame = page.locator("#preview-frame");
  await expect(frame).toBeVisible();
  const box = await frame.boundingBox();
  expect(box).not.toBeNull();
  const qr = page.locator("#preview-qr");
  const qrBox = await qr.boundingBox();
  expect(qrBox).not.toBeNull();
  if (box && qrBox) {
    expect(qrBox.x).toBeGreaterThanOrEqual(box.x - 1);
    expect(qrBox.y).toBeGreaterThanOrEqual(box.y - 1);
    expect(qrBox.x + qrBox.width).toBeLessThanOrEqual(box.x + box.width + 1);
    expect(qrBox.y + qrBox.height).toBeLessThanOrEqual(box.y + box.height + 1);
  }
});