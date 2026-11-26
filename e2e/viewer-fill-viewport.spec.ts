import { expect, test, type Page } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({ locale: "es-CL", extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" } });

const VIEWPORTS = [
  { width: 1280, height: 800, label: "desktop 1280x800" },
  { width: 1920, height: 1080, label: "desktop 1920x1080" },
  { width: 3440, height: 1440, label: "ultrawide 3440x1440" },
  { width: 390, height: 844, label: "mobile 390x844" },
];

async function assertViewerFillsViewport(page: Page, vw: number, vh: number): Promise<void> {
  const containerBox = (await page.locator("#cesiumContainer").boundingBox())!;
  expect(containerBox, "#cesiumContainer bounding box").not.toBeNull();
  expect(Math.abs(containerBox.x)).toBeLessThanOrEqual(2);
  expect(Math.abs(containerBox.y)).toBeLessThanOrEqual(2);
  expect(Math.abs(containerBox.width - vw)).toBeLessThanOrEqual(2);
  expect(Math.abs(containerBox.height - vh)).toBeLessThanOrEqual(2);

  const canvas = page.locator("#cesiumContainer canvas");
  if ((await canvas.count()) > 0) {
    const canvasBox = (await canvas.first().boundingBox())!;
    expect(canvasBox, "Cesium canvas bounding box").not.toBeNull();
    expect(Math.abs(canvasBox.x)).toBeLessThanOrEqual(2);
    expect(Math.abs(canvasBox.y)).toBeLessThanOrEqual(2);
    expect(Math.abs(canvasBox.width - vw)).toBeLessThanOrEqual(2);
    expect(Math.abs(canvasBox.height - vh)).toBeLessThanOrEqual(2);
  }

  const uiBox = (await page.locator("#ui").boundingBox())!;
  expect(uiBox, "#ui bounding box").not.toBeNull();
  expect(Math.abs(uiBox.x)).toBeLessThanOrEqual(2);
  expect(Math.abs(uiBox.y)).toBeLessThanOrEqual(2);
  expect(Math.abs(uiBox.width - vw)).toBeLessThanOrEqual(2);
  expect(Math.abs(uiBox.height - vh)).toBeLessThanOrEqual(2);

  const edgeTol = 12;

  const topbarBox = (await page.locator(".topbar").boundingBox())!;
  expect(topbarBox, ".topbar bounding box").not.toBeNull();
  expect(topbarBox.y, ".topbar near top edge").toBeLessThanOrEqual(edgeTol);
  expect(topbarBox.x, ".topbar near left edge").toBeLessThanOrEqual(edgeTol);
  expect(vw - (topbarBox.x + topbarBox.width), ".topbar stretches to right edge").toBeLessThanOrEqual(edgeTol);

  const transportBox = (await page.locator(".transport").boundingBox())!;
  expect(transportBox, ".transport bounding box").not.toBeNull();
  expect(transportBox.x, ".transport near left edge").toBeLessThanOrEqual(edgeTol);
  expect(vw - (transportBox.x + transportBox.width), ".transport stretches to right edge").toBeLessThanOrEqual(edgeTol);
  expect(vh - (transportBox.y + transportBox.height), ".transport near bottom edge").toBeLessThanOrEqual(edgeTol);

  const hud = page.locator("#hud");
  await expect(hud).toBeVisible();
  const hudBox = (await hud.boundingBox())!;
  expect(hudBox, "#hud bounding box").not.toBeNull();

  if (vw >= 1024) {
    // Desktop: HUD anchored at top-right, right below top-bar actions
    const topActionsBox = (await page.locator(".top-actions").boundingBox())!;
    expect(vw - (hudBox.x + hudBox.width), "desktop #hud hugs right edge").toBeLessThanOrEqual(edgeTol);
    expect(hudBox.y, "desktop #hud sits below .top-actions").toBeGreaterThanOrEqual(
      topActionsBox.y + topActionsBox.height - 1,
    );
    expect(hudBox.y, "desktop #hud stays near top").toBeLessThanOrEqual(96);
  } else {
    // Mobile: HUD sits directly under the top bar, stretching edge-to-edge
    expect(hudBox.x, "mobile #hud near left edge").toBeLessThanOrEqual(edgeTol);
    expect(vw - (hudBox.x + hudBox.width), "mobile #hud near right edge").toBeLessThanOrEqual(edgeTol);
    expect(hudBox.y, "mobile #hud sits below .topbar").toBeGreaterThanOrEqual(
      topbarBox.y + topbarBox.height - 1,
    );
    expect(hudBox.y - (topbarBox.y + topbarBox.height), "mobile #hud gap below .topbar").toBeLessThanOrEqual(16);
  }

  // No horizontal or vertical page overflow
  const dims = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
    scrollHeight: document.documentElement.scrollHeight,
    clientHeight: document.documentElement.clientHeight,
  }));
  expect(dims.scrollWidth).toBeLessThanOrEqual(dims.clientWidth);
  expect(dims.scrollHeight).toBeLessThanOrEqual(dims.clientHeight);
}

for (const vp of VIEWPORTS) {
  test(`flight viewer fills viewport at ${vp.label}`, async ({ page }) => {
    await page.setViewportSize({ width: vp.width, height: vp.height });
    await stubArcgisTiles(page);
    await page.goto("/vuelo-5");

    await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
    await expect(page.locator("#hud")).toBeVisible({ timeout: 15_000 });

    await assertViewerFillsViewport(page, vp.width, vp.height);
  });
}

test("flight viewer keeps filling viewport across window resizes", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await stubArcgisTiles(page);
  await page.goto("/vuelo-5");

  await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
  await expect(page.locator("#hud")).toBeVisible({ timeout: 15_000 });

  await assertViewerFillsViewport(page, 1280, 800);

  for (const next of [
    { width: 1920, height: 1080 },
    { width: 390, height: 844 },
    { width: 1600, height: 900 },
  ]) {
    await page.setViewportSize(next);
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    await assertViewerFillsViewport(page, next.width, next.height);
  }
});
