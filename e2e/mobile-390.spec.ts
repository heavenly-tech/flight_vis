import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

type Box = { x: number; y: number; width: number; height: number };

function boxesOverlap(a: Box, b: Box, tolerance = 0.5): boolean {
  return !(
    a.x + a.width <= b.x + tolerance ||
    b.x + b.width <= a.x + tolerance ||
    a.y + a.height <= b.y + tolerance ||
    b.y + b.height <= a.y + tolerance
  );
}

test.use({ viewport: { width: 390, height: 844 }, locale: "es-CL" });

test("globe top bar at 390x844 has no overlapping actions/title and hides wordmark text", async ({
  page,
}) => {
  await stubArcgisTiles(page);
  await page.goto("/");

  const loginBtn = page.locator("#btn-login");
  const libraryBtn = page.locator("#btn-library");
  const fileBtn = page.locator("#btn-file");
  const flightName = page.locator("#flight-name");
  const brandLockup = page.locator(".brand-lockup");
  const brandLogo = page.locator(".brand-logo");
  const brandWordmark = page.locator(".brand");
  const transport = page.locator(".transport");

  await expect(loginBtn).toBeVisible({ timeout: 15_000 });
  await expect(libraryBtn).toBeVisible();
  await expect(fileBtn).toBeVisible();
  await expect(brandLogo).toBeVisible();
  await expect(flightName).toBeVisible();
  await expect(flightName).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });

  // Wordmark text is hidden on narrow mobile viewports (icon stays visible)
  await expect(brandWordmark).toBeHidden();

  const items = [
    { name: "brand-lockup", box: (await brandLockup.boundingBox())! },
    { name: "btn-login", box: (await loginBtn.boundingBox())! },
    { name: "btn-library", box: (await libraryBtn.boundingBox())! },
    { name: "btn-file", box: (await fileBtn.boundingBox())! },
    { name: "flight-name", box: (await flightName.boundingBox())! },
  ];

  for (const item of items) {
    expect(item.box, `${item.name} bounding box`).not.toBeNull();
    expect(item.box.x, `${item.name} left >= 0`).toBeGreaterThanOrEqual(-0.5);
    expect(item.box.x + item.box.width, `${item.name} right <= 390`).toBeLessThanOrEqual(390.5);
  }

  // No pairwise overlaps among topbar logo, actions, and flight title
  for (let i = 0; i < items.length; i++) {
    for (let j = i + 1; j < items.length; j++) {
      expect(
        boxesOverlap(items[i].box, items[j].box),
        `${items[i].name} overlaps ${items[j].name}`,
      ).toBe(false);
    }
  }

  // Flight title gets full row width and sits below the top action row
  const titleBox = items.find((i) => i.name === "flight-name")!.box;
  expect(titleBox.width).toBeGreaterThan(250);

  // Bottom transport fits inside the 390x844 viewport with safe bottom margin
  const transportBox = (await transport.boundingBox())!;
  expect(transportBox).not.toBeNull();
  expect(transportBox.x + transportBox.width).toBeLessThanOrEqual(390.5);
  expect(transportBox.y + transportBox.height).toBeLessThanOrEqual(836);

  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
});

test("/app.html at 390x844 wraps SHA-256 inside card, aligns CTA buttons, and has no horizontal scroll", async ({
  page,
}) => {
  await page.route("**/app/app-release.json", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        available: true,
        version: "1.2.3",
        sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        apkUrl: "/app/flightvis.apk",
        message: "Versión de prueba.",
      }),
    });
  });
  await page.route("/app/flightvis.apk", async (route) => {
    if (route.request().method() === "HEAD") {
      await route.fulfill({ status: 200 });
    } else {
      await route.continue();
    }
  });
  await page.route("**/api/app/install-link", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        httpsUrl: "https://example.test/app.html",
        apkUrl: "/app/flightvis.apk",
        intentUrl:
          "intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;package=cl.heavenly.flightvis;S.browser_fallback_url=https%3A%2F%2Fexample.test%2Fapp%2Fflightvis.apk;end",
      }),
    });
  });

  await page.goto("/app.html", { waitUntil: "domcontentloaded" });

  const card = page.locator("#release-card");
  const shaVal = page.locator("#sha-val");
  const downloadBtn = page.locator("#download");
  const openInAppBtn = page.locator("#open-in-app");

  await expect(shaVal).toBeVisible({ timeout: 15_000 });
  await expect(downloadBtn).toBeVisible({ timeout: 15_000 });
  await expect(openInAppBtn).toBeVisible({ timeout: 15_000 });

  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);

  const cardBox = (await card.boundingBox())!;
  const shaBox = (await shaVal.boundingBox())!;
  const dlBox = (await downloadBtn.boundingBox())!;
  const openBox = (await openInAppBtn.boundingBox())!;

  // SHA-256 element stays within its card bounds
  expect(shaBox.x).toBeGreaterThanOrEqual(cardBox.x - 1);
  expect(shaBox.x + shaBox.width).toBeLessThanOrEqual(cardBox.x + cardBox.width + 1);

  // CTA buttons stay inside card, do not overlap, and align cleanly (same row OR flush-left wrapped row)
  expect(dlBox.x + dlBox.width).toBeLessThanOrEqual(cardBox.x + cardBox.width + 1);
  expect(openBox.x + openBox.width).toBeLessThanOrEqual(cardBox.x + cardBox.width + 1);
  expect(boxesOverlap(dlBox, openBox)).toBe(false);

  const sameRow = Math.abs(dlBox.y - openBox.y) <= 2;
  const alignedWrappedRow =
    Math.abs(dlBox.x - openBox.x) <= 1 && openBox.y >= dlBox.y + dlBox.height - 1;
  expect(sameRow || alignedWrappedRow).toBe(true);
});

test("/c/demo/registro at 390x844 keeps language select compact (not full-width)", async ({
  page,
}) => {
  await page.goto("/c/demo/registro");

  const langSelect = page.locator("#lang-slot select");
  const nameInput = page.locator('form#form input[name="name"]');

  await expect(langSelect).toBeVisible({ timeout: 15_000 });
  await expect(nameInput).toBeVisible({ timeout: 15_000 });

  const selectBox = (await langSelect.boundingBox())!;
  const inputBox = (await nameInput.boundingBox())!;

  expect(selectBox.width).toBeLessThan(inputBox.width * 0.7);
  expect(selectBox.width).toBeLessThan(220);
  expect(inputBox.width).toBeGreaterThan(260);

  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
});
