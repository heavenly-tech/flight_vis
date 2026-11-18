import { test, expect } from "@playwright/test";

test("signup page is available", async ({ page }) => {
  const res = await page.goto("/signup");
  expect(res?.status()).toBeLessThan(400);
  await expect(page.getByRole("heading", { name: /crea tu cuenta|create your account/i })).toBeVisible();
});

test("signup capability endpoint", async ({ request }) => {
  const res = await request.get("/api/account/signup-available");
  expect(res.ok()).toBeTruthy();
  const body = await res.json();
  expect(body.signup).toBe(true);
});