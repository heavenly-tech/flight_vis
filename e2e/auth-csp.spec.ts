import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";
import { installPasswordUrlGuard } from "./auth-csp-helpers";
import { HTML_PAGE_CSP } from "./csp-constants";
import {
  assertNoPasswordInUrl as assertLocationHasNoPassword,
  trackPasswordInRequestUrls,
} from "./club-login-helpers";

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
    __fvAccountSignupReady?: boolean;
  }
}

async function assertNoPasswordInUrl(page: import("@playwright/test").Page) {
  expect(page.url()).not.toMatch(/[?&]password=/i);
}

test.describe("credential pages under prod CSP", () => {
  test.setTimeout(60_000);

  test("unified login password never leaks to URL", async ({ page }) => {
    const leaks = installPasswordUrlGuard(page);
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login?mode=password&next=/club-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    await page.locator("#login-form input[name=email]").waitFor({ state: "visible" });
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#submit-password").click();
    await page.waitForTimeout(1500);
    await assertNoPasswordInUrl(page);
    expect(leaks).toEqual([]);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("unified login without JS never leaks password in request URLs", async ({ browser }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    const leaks = installPasswordUrlGuard(page);
    await page.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#submit-password").click();
    await page.waitForTimeout(1500);
    expect(leaks).toEqual([]);
    await context.close();
  });

  test("club-login.html redirects to /login with prod CSP", async ({ page }) => {
    const res = await page.goto("/club-login.html?next=%2Fclub-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    expect(res?.url()).toMatch(/\/login/);
    const csp = res?.headers()["content-security-policy"];
    expect(csp?.trim()).toBe(HTML_PAGE_CSP);
  });

  test("no-JS legacy club form post never leaks password in URLs", async ({
    browser,
  }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    const leak = trackPasswordInRequestUrls(page);
    const res = await page.request.post("/api/club/login", {
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "sec-fetch-site": "same-origin",
      },
      form: {
        email: "admin@demo.test",
        password: "password12",
        next: "/club-pilot.html",
      },
      maxRedirects: 0,
      failOnStatusCode: false,
    });
    const loc = res.headers().location ?? "";
    assertLocationHasNoPassword(loc);
    leak.assertClean();
    await context.close();
  });

  test("magic link request never puts password in URL", async ({ page }) => {
    const leaks = installPasswordUrlGuard(page);
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await page.locator("#email").waitFor({ state: "visible" });
    await page.locator("#submit-magic").waitFor({ state: "visible" });
    await page.locator("#email").fill("admin@demo.test");
    await page.locator("#submit-magic").click();
    await page.waitForTimeout(300);
    await assertNoPasswordInUrl(page);
    expect(leaks).toEqual([]);
  });

  test("legacy /signup route redirects to pilot register without password in URL", async ({
    page,
  }) => {
    const leaks = installPasswordUrlGuard(page);
    const { consoleErrors } = installCspObservers(page);
    const email = `csp-signup-${Date.now()}@example.test`;
    await page.goto("/signup", { waitUntil: "domcontentloaded" });
    await expect(page).toHaveURL(/\/(club-register\.html|c\/[^/]+\/registro)/);
    await page.locator("#form input[name=name]").waitFor({ state: "visible" });
    await page.locator("#form input[name=name]").fill("E2E Signup");
    await page.locator("#form input[name=email]").fill(email);
    await page.locator("#form input[name=password]").fill("password1234");
    await page.locator("#form button[type=submit]").click();
    await page.waitForTimeout(500);
    await assertNoPasswordInUrl(page);
    expect(leaks).toEqual([]);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("club login forms never put password in URL", async ({ page }) => {
    const leaks = installPasswordUrlGuard(page);
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
    await page.locator("#legacy-details").click();
    await page.locator("#login-form input[name=email]").fill("admin@demo.test");
    await page.locator("#login-form input[name=password]").fill("password12");
    await page.locator("#legacy-submit").click();
    await page.waitForTimeout(800);
    await assertNoPasswordInUrl(page);
    expect(leaks).toEqual([]);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });

  test("club register form uses POST without password in URL", async ({ page }) => {
    const leaks = installPasswordUrlGuard(page);
    await page.goto("/club-register.html", { waitUntil: "domcontentloaded" });
    const email = `reg-${Date.now()}@example.test`;
    await page.locator('form input[name="email"]').fill(email);
    await page.locator('form input[name="password"]').fill("password1234");
    await page.locator('form input[name="name"]').fill("E2E Reg");
    await page.locator("form button[type=submit]").click();
    await page.waitForTimeout(800);
    await assertNoPasswordInUrl(page);
    expect(leaks).toEqual([]);
  });

  test("club signup form uses POST without secrets in URL", async ({ page }) => {
    const leaks = installPasswordUrlGuard(page);
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
    expect(leaks).toEqual([]);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});