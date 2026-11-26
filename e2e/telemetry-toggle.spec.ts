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

test.use({ locale: "es-CL", extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" } });

test("telemetry toggle in More hides/shows HUD, persists across reload, and respects shortcut focus rules", async ({
  page,
}) => {
  await stubArcgisTiles(page);
  await page.goto("/vuelo-5");

  const hud = page.locator("#hud");
  const moreBtn = page.locator("#btn-more");
  const toggleBtn = page.locator("#btn-telemetry");

  await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
  await expect(hud).toBeVisible({ timeout: 15_000 });

  // Open More and verify accessible toggle state
  await moreBtn.click();
  await expect(toggleBtn).toBeVisible();
  await expect(toggleBtn).toHaveText("Telemetría");
  await expect(toggleBtn).toHaveAttribute("aria-label", "Mostrar u ocultar las lecturas de vuelo");
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "true");

  // Hide telemetry cluster
  await toggleBtn.click();
  await expect(hud).toBeHidden();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "false");
  expect(await page.evaluate(() => localStorage.getItem("fv.telemetryVisible"))).toBe("false");

  // Reload and confirm persistence: flight loads, HUD stays hidden
  await page.reload();
  await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
  await expect(page.locator("#summary")).not.toBeEmpty({ timeout: 15_000 });
  await expect(hud).toBeHidden();
  await moreBtn.click();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "false");

  // Show it again via toggle button
  await toggleBtn.click();
  await expect(hud).toBeVisible();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "true");

  // Keyboard shortcut: 'h' toggles when no field is focused
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("h");
  await expect(hud).toBeHidden();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "false");

  // Shift+H ('H') restores it
  await page.keyboard.press("Shift+H");
  await expect(hud).toBeVisible();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "true");

  // Modifier keys (Control+h / Alt+h) do NOT toggle
  await page.keyboard.press("Control+h");
  await expect(hud).toBeVisible();
  await page.keyboard.press("Alt+h");
  await expect(hud).toBeVisible();

  // Focus in #play-speed (<select> inside open More panel): 'h' must NOT toggle
  const speedSelect = page.locator("#play-speed");
  await speedSelect.focus();
  await page.keyboard.press("h");
  await expect(hud).toBeVisible();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "true");

  // Open File menu and focus #slug-input: 'h' must NOT toggle telemetry
  await page.locator("#btn-file").click();
  const slugInput = page.locator("#slug-input");
  await expect(slugInput).toBeVisible();
  await slugInput.focus();
  await page.keyboard.press("h");
  await expect(hud).toBeVisible();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "true");
});

test("telemetry toggle at 390x844 has >= 44x44 touch target and zero overlaps", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubArcgisTiles(page);
  await page.goto("/vuelo-5");

  await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
  await expect(page.locator("#hud")).toBeVisible({ timeout: 15_000 });

  const moreBtn = page.locator("#btn-more");
  const toggleBtn = page.locator("#btn-telemetry");
  const langBtn = page.locator("#btn-lang");
  const topbar = page.locator(".topbar");
  const transport = page.locator(".transport");
  const primaryRow = page.locator(".transport-primary");
  const mobileColor = page.locator(".mobile-color");

  await moreBtn.click();
  await expect(toggleBtn).toBeVisible();

  const toggleBox = (await toggleBtn.boundingBox())!;
  const moreBox = (await moreBtn.boundingBox())!;
  const langBox = (await langBtn.boundingBox())!;
  const topbarBox = (await topbar.boundingBox())!;
  const transportBox = (await transport.boundingBox())!;
  const primaryRowBox = (await primaryRow.boundingBox())!;
  const mobileColorBox = (await mobileColor.boundingBox())!;

  expect(toggleBox).not.toBeNull();
  expect(toggleBox.width, "touch target width >= 44px").toBeGreaterThanOrEqual(44);
  expect(toggleBox.height, "touch target height >= 44px").toBeGreaterThanOrEqual(44);

  // Stays inside viewport and inside transport drawer bounds
  expect(toggleBox.x).toBeGreaterThanOrEqual(-0.5);
  expect(toggleBox.x + toggleBox.width).toBeLessThanOrEqual(390.5);
  expect(toggleBox.y).toBeGreaterThanOrEqual(transportBox.y - 0.5);
  expect(toggleBox.y + toggleBox.height).toBeLessThanOrEqual(transportBox.y + transportBox.height + 0.5);

  // Does not overlap #btn-more, primary transport row, #btn-lang, .mobile-color, or .topbar
  expect(boxesOverlap(toggleBox, moreBox), "overlaps #btn-more").toBe(false);
  expect(boxesOverlap(toggleBox, primaryRowBox), "overlaps .transport-primary").toBe(false);
  expect(boxesOverlap(toggleBox, langBox), "overlaps #btn-lang").toBe(false);
  expect(boxesOverlap(toggleBox, mobileColorBox), "overlaps .mobile-color").toBe(false);
  expect(boxesOverlap(toggleBox, topbarBox), "overlaps .topbar").toBe(false);
  expect(boxesOverlap(transportBox, topbarBox), "transport drawer overlaps .topbar").toBe(false);

  // No horizontal scroll
  const overflow = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);

  // Toggling hides HUD cleanly on mobile
  await toggleBtn.click();
  await expect(page.locator("#hud")).toBeHidden();
  await expect(toggleBtn).toHaveAttribute("aria-pressed", "false");
});
