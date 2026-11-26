import { expect, test } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("kiosk layout uses decorative inline QR placeholder with no 401 request, stays inside preview box, and links to club screen", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const screenQrRequests: string[] = [];
  page.on("request", (req) => {
    if (req.url().includes("/api/club/screen/qr.svg")) {
      screenQrRequests.push(req.url());
    }
  });

  await page.goto("/club-admin-kiosk-layout.html", { waitUntil: "networkidle" });

  // No request to display-only /api/club/screen/qr.svg
  expect(screenQrRequests).toEqual([]);
  await expect(page.locator("#preview-qr img")).toHaveCount(0);

  const frame = page.locator("#preview-frame");
  await expect(frame).toBeVisible();

  const qrSvg = page.locator('#preview-qr svg[data-testid="qr-placeholder"]');
  await expect(qrSvg).toBeVisible();
  await expect(qrSvg).toHaveAttribute("aria-hidden", "true");

  const box = await frame.boundingBox();
  expect(box).not.toBeNull();
  const qrBox = await qrSvg.boundingBox();
  expect(qrBox).not.toBeNull();
  if (box && qrBox) {
    expect(qrBox.width).toBeGreaterThan(20);
    expect(qrBox.height).toBeGreaterThan(20);
    expect(qrBox.x).toBeGreaterThanOrEqual(box.x - 1);
    expect(qrBox.y).toBeGreaterThanOrEqual(box.y - 1);
    expect(qrBox.x + qrBox.width).toBeLessThanOrEqual(box.x + box.width + 1);
    expect(qrBox.y + qrBox.height).toBeLessThanOrEqual(box.y + box.height + 1);
  }

  // Dragging #preview-qr updates position cleanly
  await frame.scrollIntoViewIfNeeded();
  const initialX = Number(await page.locator("#qr-x").inputValue());
  const previewQrBox = (await page.locator("#preview-qr").boundingBox())!;
  const startX = previewQrBox.x + previewQrBox.width / 2;
  const startY = previewQrBox.y + previewQrBox.height / 2;
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX - 80, startY - 40, { steps: 5 });
  await page.mouse.up();
  const movedX = Number(await page.locator("#qr-x").inputValue());
  expect(movedX).toBeLessThan(initialX);

  // Resizing via #resize-handle updates size cleanly
  const initialSize = Number(await page.locator("#qr-size").inputValue());
  const handleBox = (await page.locator("#resize-handle").boundingBox())!;
  expect(handleBox).not.toBeNull();
  const hX = handleBox.x + handleBox.width / 2;
  const hY = handleBox.y + handleBox.height / 2;
  await page.mouse.move(hX, hY);
  await page.mouse.down();
  await page.mouse.move(hX + 40, hY + 40, { steps: 5 });
  await page.mouse.up();
  const resizedSize = Number(await page.locator("#qr-size").inputValue());
  expect(resizedSize).toBeGreaterThan(initialSize);

  // Visible "Abrir pantalla del club" link
  const kioskLink = page.locator("a[data-open-kiosk-link]");
  await expect(kioskLink).toBeVisible();
  await expect(kioskLink).toHaveText("Abrir pantalla del club");
  await expect(kioskLink).toHaveAttribute("href", "/c/demo/screen");
  await expect(kioskLink).toHaveAttribute("target", "_blank");
  await expect(kioskLink).toHaveAttribute("rel", "noopener");

  expect(screenQrRequests).toEqual([]);
});
