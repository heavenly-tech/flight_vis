import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
    __fvAccountSignupReady?: boolean;
  }
}

test.describe("credential pages under prod CSP", () => {
  test.setTimeout(60_000);

  test("unified login password never leaks to URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await Promise.all([
      page.waitForURL(/account-clubs\.html/, { timeout: 20_000 }),
      page.locator("#login-form button[type=submit]").click(),
    ]);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("magic link request never puts password in URL", async ({ page }) => {
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#magic-btn").click();
    await page.waitForTimeout(300);
    expect(page.url()).not.toMatch(/password/i);
  });

  test("account signup form never puts password in URL", async ({ page }) => {
    const email = `csp-signup-${Date.now()}@example.test`;
    await page.goto("/signup", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvAccountSignupReady === true);
    await page.locator("#form input[name=email]").fill(email);
    await page.locator("#form input[name=password]").fill("password1234");
    await page.locator("#form button[type=submit]").click();
    await page.waitForTimeout(500);
    expect(page.url()).not.toMatch(/password/i);
  });
});