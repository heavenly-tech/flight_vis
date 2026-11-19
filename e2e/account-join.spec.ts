import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

test.describe("account join club", () => {
  test.setTimeout(60_000);

  test("pending join via slug field does not leak secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await Promise.all([
      page.waitForURL(/account-clubs\.html/, { timeout: 20_000 }),
      page.locator("#login-form button[type=submit]").click(),
    ]);
    await page.goto("/account-clubs.html", { waitUntil: "domcontentloaded" });
    const joinForm = page.locator("#join-form");
    await joinForm.locator('input[name="slug"]').fill("demo");
    await joinForm.locator("button[type=submit]").click();
    await page.waitForTimeout(800);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});