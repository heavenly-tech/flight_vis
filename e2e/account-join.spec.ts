import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

test.describe("account join club", () => {
  test.setTimeout(90_000);

  test("pending join via slug field does not leak secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login?next=/account-clubs.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#login-form button[type=submit]").click();
    const reachedClubs = await page
      .waitForURL(/account-clubs\.html/, { timeout: 25_000 })
      .then(() => true)
      .catch(() => false);
    if (!reachedClubs) {
      const login = await page.request.post("/api/account/login", {
        data: { email: "admin@demo.test", password: "password12" },
      });
      expect(login.ok()).toBeTruthy();
      await page.goto("/account-clubs.html", { waitUntil: "load" });
    }
    await page.locator("#join-open").click();
    const joinForm = page.locator("#join-form");
    await expect(joinForm).toBeVisible({ timeout: 10_000 });
    await joinForm.locator('input[name="slug"]').fill("demo");
    await joinForm.locator("button[type=submit]").click();
    await page.waitForTimeout(800);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});