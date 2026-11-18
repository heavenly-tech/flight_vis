import { test, expect } from "@playwright/test";

test("club login page loads", async ({ page }) => {
  await page.goto("/club-login.html?mode=pilot", { waitUntil: "domcontentloaded" });
  await expect(page.locator("#login-heading")).toContainText(/sesión|login|pilot|club/i);
  await expect(page.locator("#form input[name=email]")).toBeVisible();
});