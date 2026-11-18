import { expect, test } from "@playwright/test";

test("root with one public live flight opens that flight", async ({ page }) => {
  await page.route("**/api/public/live-summary*", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ flights: [{ flightId: "live-one", tail: "N1" }] }),
    });
  });

  await page.goto("/");
  await expect(page).toHaveURL(/club-flight\.html\?id=live-one/, { timeout: 15_000 });
});

test("/map skips live-summary redirect", async ({ page }) => {
  let summaryHits = 0;
  await page.route("**/api/public/live-summary*", async (route) => {
    summaryHits += 1;
    await route.fulfill({ status: 200, body: JSON.stringify({ flights: [{ flightId: "x", tail: "N1" }] }) });
  });

  await page.goto("/map");
  await expect(page).toHaveURL(/\/map\/?$/, { timeout: 15_000 });
  expect(summaryHits).toBe(0);
});