import { test, expect } from "@playwright/test";

test("club login page loads", async ({ page }) => {
  await page.goto("/club/login");
  await expect(page.locator("h1")).toContainText(/login|pilot|club/i);
  await expect(page.locator('input[type="email"], input[name="email"]')).toBeVisible();
});