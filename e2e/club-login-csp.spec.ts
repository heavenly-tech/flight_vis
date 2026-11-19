import { test, expect } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";
import { HTML_PAGE_CSP } from "./csp-constants";
import {
  assertNoPasswordInUrl,
  trackPasswordInRequestUrls,
} from "./club-login-helpers";

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

test.describe("club login under prod CSP", () => {
  test.setTimeout(60_000);

  test("serves Rust HTML_CSP on club-login.html", async ({ page }) => {
    const res = await page.goto("/club-login.html", {
      waitUntil: "domcontentloaded",
    });
    const csp = res?.headers()["content-security-policy"];
    expect(csp?.trim()).toBe(HTML_PAGE_CSP);
  });

  test("JS login never puts password in any request URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    const leak = trackPasswordInRequestUrls(page);
    await page.goto("/club-login.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvClubLoginReady === true);
    await page.fill('input[name="email"]', "admin@demo.test");
    await page.fill('input[name="password"]', "password12");
    await Promise.all([
      page.waitForURL(/club-pilot\.html/),
      page.locator("#form button[type=submit]").click(),
    ]);
    assertNoPasswordInUrl(page.url());
    leak.assertClean();
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual(
      [],
    );
  });

  test("no-JS form post never leaks password in URLs", async ({ browser }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    const leak = trackPasswordInRequestUrls(page);
    try {
      await page.goto("/club-login.html", { waitUntil: "domcontentloaded" });
      await page.fill('input[name="email"]', "admin@demo.test");
      await page.fill('input[name="password"]', "password12");
      await Promise.all([
        page.waitForURL(/club-pilot\.html/, { timeout: 30_000 }),
        page.locator("#form button[type=submit]").click(),
      ]);
      assertNoPasswordInUrl(page.url());
      leak.assertClean();
    } finally {
      await context.close();
    }
  });
});