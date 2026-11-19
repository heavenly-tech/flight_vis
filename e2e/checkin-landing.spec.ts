import { test, expect } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("checkin expired page for invalid token", async ({ page }) => {
  await page.goto("/c/demo/q/invalid-token-value");
  await expect(page.locator("h1")).toContainText(/no válido/i, {
    timeout: 15_000,
  });
});
