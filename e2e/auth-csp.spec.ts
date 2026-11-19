import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
    __fvAccountSignupReady?: boolean;
  }
}

async function assertNoPasswordInUrl(page: import("@playwright/test").Page) {
  expect(page.url()).not.toMatch(/password/i);
}

test.describe("credential pages under prod CSP", () => {
  test.setTimeout(60_000);

  test("unified login password never leaks to URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login?next=/club-pilot.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#login-form button[type=submit]").click();
    await page.waitForTimeout(1500);
    await assertNoPasswordInUrl(page);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("magic link request never puts password in URL", async ({ page }) => {
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#magic-btn").click();
    await page.waitForTimeout(300);
    await assertNoPasswordInUrl(page);
  });

  test("account signup form never puts password in URL", async ({ page }) => {
    const email = `csp-signup-${Date.now()}@example.test`;
    await page.goto("/signup", { waitUntil: "domcontentloaded" });
    await page.locator("#form input[name=email]").waitFor({ state: "visible" });
    await page.locator("#form input[name=email]").fill(email);
    await page.locator("#form input[name=password]").fill("password1234");
    await page.locator("#form button[type=submit]").click();
    await page.waitForTimeout(500);
    await assertNoPasswordInUrl(page);
  });

  test("club login forms never put password in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/club-login.html?mode=pilot", { waitUntil: "domcontentloaded" });
    await page.locator("#form input[name=email]").waitFor({ state: "visible" });
    await page.locator("#form input[name=email]").fill("admin@demo.test");
    await page.locator("#form input[name=password]").fill("password12");
    const submit = page.locator("#form button[type=submit]");
    await submit.click();
    await page.waitForTimeout(800);
    await assertNoPasswordInUrl(page);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("club register form uses POST without password in URL", async ({ page }) => {
    await page.goto("/club-register.html", { waitUntil: "domcontentloaded" });
    const email = `reg-${Date.now()}@example.test`;
    await page.locator('form input[name="email"]').fill(email);
    await page.locator('form input[name="password"]').fill("password1234");
    await page.locator('form input[name="name"]').fill("E2E Reg");
    await page.locator("form button[type=submit]").click();
    await page.waitForTimeout(800);
    await assertNoPasswordInUrl(page);
  });

  test("club signup form uses POST without secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/club-signup.html", { waitUntil: "domcontentloaded" });
    const stamp = Date.now();
    const email = `club-signup-${stamp}@example.test`;
    await page.locator('form input[name="name"]').fill("E2E Club");
    await page.locator('form input[name="slug"]').fill(`e2e-club-${stamp}`);
    await page.locator('form input[name="email"]').fill(email);
    await page.locator("form button[type=submit]").click();
    await page.waitForTimeout(800);
    await assertNoPasswordInUrl(page);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});