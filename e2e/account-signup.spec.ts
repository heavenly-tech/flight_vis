import { test, expect } from "@playwright/test";

test("signup routes redirect to the unified pilot registration flow", async ({ page }) => {
  for (const legacyPath of ["/signup", "/signup.html"]) {
    const res = await page.goto(legacyPath);
    expect(res?.status()).toBeLessThan(400);
    await expect(page).toHaveURL(/\/(club-register\.html|c\/[^/]+\/registro)/);
    await expect(
      page.getByRole("heading", { name: /registro|pilot registration|register/i }),
    ).toBeVisible();
  }
});

test("signup capability endpoint", async ({ request }) => {
  const res = await request.get("/api/account/signup-available");
  expect(res.ok()).toBeTruthy();
  const body = await res.json();
  expect(body.signup).toBe(true);
});