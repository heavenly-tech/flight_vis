import { test, expect } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";
import { HTML_PAGE_CSP } from "./csp-constants";
import {
  assertNoPasswordInUrl,
  trackPasswordInRequestUrls,
} from "./club-login-helpers";

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
  }
}

test.describe("login under prod CSP", () => {
  test.setTimeout(60_000);

  test("club-login.html redirects to /login with CSP on final page", async ({
    page,
  }) => {
    const res = await page.goto("/club-login.html?next=%2Fclub-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    expect(res?.url()).toMatch(/\/login/);
    const csp = res?.headers()["content-security-policy"];
    expect(csp?.trim()).toBe(HTML_PAGE_CSP);
  });

  test("JS legacy club login never puts password in any request URL", async ({
    page,
  }) => {
    const { consoleErrors } = installCspObservers(page);
    const leak = trackPasswordInRequestUrls(page);
    await page.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvUnifiedLoginReady === true);
    await page.locator("details").setAttribute("open", "");
    await page.fill('#legacy-club-form input[name="email"]', "admin@demo.test");
    await page.fill('#legacy-club-form input[name="password"]', "password12");
    await Promise.all([
      page.waitForURL(/club-pilot\.html/),
      page.locator('#legacy-club-form button[type=submit]').click(),
    ]);
    assertNoPasswordInUrl(page.url());
    leak.assertClean();
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual(
      [],
    );
  });

  test("no-JS legacy form post never leaks password in URLs", async ({
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
    assertNoPasswordInUrl(loc);
    leak.assertClean();
    await context.close();
  });
});