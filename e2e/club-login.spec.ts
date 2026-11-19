import { test, expect } from "@playwright/test";

test("club-login.html redirects to unified login", async ({ page }) => {
  const res = await page.goto("/club-login.html?mode=pilot&next=%2Fclub-pilot.html", {
    waitUntil: "domcontentloaded",
  });
  expect(res?.url()).toMatch(/\/login/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-pilot.html");
});