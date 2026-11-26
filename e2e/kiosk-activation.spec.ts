import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("signed-out user is redirected from club-screen to login", async ({ page }) => {
  await page.goto("/club-screen.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/\/login\?/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-screen.html");
});

test("signed-in admin sees kiosk immediately, polls aircraft with no reload, and has no activation form", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await stubArcgisTiles(page);

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
  await expect(page.locator("#activate")).toHaveCount(0);
  await expect(page.locator("#activate-token")).toHaveCount(0);
  await expect(page.locator("#btn-local-qr")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#status-bar")).toContainText(/rota cada \d+ segundos/i);

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

  await page.evaluate(() => {
    (window as unknown as { __fvNoReload?: string }).__fvNoReload = "same-doc";
  });

  await expect(page.locator("#map")).toContainText("CC-AAA · Piloto Uno");
  const kioskMap = page.locator('[data-testid="kiosk-map"]');
  await expect(kioskMap).toBeVisible({ timeout: 10_000 });
  await expect
    .poll(() => kioskMap.locator("img[data-tile]").count(), { timeout: 10_000 })
    .toBeGreaterThan(0);

  await expect(page.locator("#map")).toContainText("CC-BBB · Piloto Dos", { timeout: 12_000 });
  expect(mapPolls).toBeGreaterThanOrEqual(2);

  const marker = await page.evaluate(
    () => (window as unknown as { __fvNoReload?: string }).__fvNoReload,
  );
  expect(marker).toBe("same-doc");
});
