import { expect, test } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("kiosk activation starts polling and reveals QR toggle with no reload", async ({
  page,
  request,
}) => {
  const login = await request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const created = await request.post("/api/admin/display-sessions", {
    data: { label: "E2E Kiosk Activation" },
  });
  expect(created.ok()).toBeTruthy();
  const { token } = (await created.json()) as { token: string };
  expect(token).toBeTruthy();

  let mapPolls = 0;
  await page.route("**/api/display/map", async (route) => {
    const res = await route.fetch();
    if (!res.ok()) {
      await route.fulfill({ response: res });
      return;
    }
    mapPolls += 1;
    const aircraft =
      mapPolls === 1
        ? [{ tail: "CC-AAA", pilotName: "Piloto Uno" }]
        : [
            { tail: "CC-AAA", pilotName: "Piloto Uno" },
            { tail: "CC-BBB", pilotName: "Piloto Dos" },
          ];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ aircraft }),
    });
  });

  await page.goto("/club-screen.html", { waitUntil: "networkidle" });

  await expect(page.locator("html")).toHaveAttribute("lang", "es-CL");
  await expect(page.locator("#activate")).toBeVisible();
  await expect(page.locator("#btn-local-qr")).toBeHidden();
  await expect(page.locator('label[for="activate-token"]')).toBeVisible();

  const headingBox = await page.locator(".kiosk-header h2").boundingBox();
  const statusBox = await page.locator("#status-bar").boundingBox();
  expect(headingBox).not.toBeNull();
  expect(statusBox).not.toBeNull();
  if (headingBox && statusBox) {
    const horizontalOverlap =
      headingBox.x < statusBox.x + statusBox.width &&
      headingBox.x + headingBox.width > statusBox.x;
    const verticalOverlap =
      headingBox.y < statusBox.y + statusBox.height &&
      headingBox.y + headingBox.height > statusBox.y;
    expect(horizontalOverlap && verticalOverlap).toBe(false);
  }

  const labelText = (await page.locator('label[for="activate-token"]').textContent())?.trim();
  const initialStatus = (await page.locator("#status-bar").textContent())?.trim();
  expect(initialStatus).not.toBe(labelText);

  await page.evaluate(() => {
    (window as unknown as { __fvNoReload?: string }).__fvNoReload = "same-doc";
  });

  await page.locator("#activate-token").fill(token);
  await page.locator("#activate-token").press("Enter");

  await expect(page.locator("#activate")).toBeHidden({ timeout: 10_000 });
  await expect(page.locator("#btn-local-qr")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#status-bar")).toContainText(/rota cada \d+ segundos/i);
  await expect(page.locator("#map")).toContainText("CC-AAA · Piloto Uno");

  await expect(page.locator("#map")).toContainText("CC-BBB · Piloto Dos", { timeout: 12_000 });
  expect(mapPolls).toBeGreaterThanOrEqual(2);

  const marker = await page.evaluate(
    () => (window as unknown as { __fvNoReload?: string }).__fvNoReload,
  );
  expect(marker).toBe("same-doc");
});
