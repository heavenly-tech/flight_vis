import { test, expect } from "@playwright/test";

test("checkin expired page for invalid token", async ({ page }) => {
  await page.goto("/c/demo/q/invalid-token-value");
  await expect(page.locator("h1")).toContainText(/no válido/i, {
    timeout: 15_000,
  });
});

test("assetlinks.json is public json", async ({ request }) => {
  const res = await request.get("/.well-known/assetlinks.json");
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toContain("application/json");
  const body = await res.json();
  expect(Array.isArray(body)).toBeTruthy();
});