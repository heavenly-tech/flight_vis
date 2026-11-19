import { test, expect } from "@playwright/test";
import {
  assertNoPasswordInUrl,
  trackPasswordInRequestUrls,
} from "./club-login-helpers";
import { HTML_PAGE_CSP } from "./csp-constants";

test.describe("unified /login", () => {
  test("club-login.html redirects to /login with next preserved", async ({
    request,
  }) => {
    const res = await request.get("/club-login.html?next=%2Fclub-pilot.html", {
      maxRedirects: 0,
    });
    expect(res.status()).toBe(302);
    const location = res.headers().location ?? "";
    expect(location).toMatch(/^\/login\?/);
    expect(location).toContain("next=%2Fclub-pilot.html");
    assertNoPasswordInUrl(location);
  });

  test("password form POST never puts password in request URL", async ({
    request,
  }) => {
    const res = await request.post("/api/account/login", {
      headers: {
        "Content-Type": "application/x-www-form-urlencoded",
        "sec-fetch-site": "same-origin",
      },
      form: {
        email: "nobody@test",
        password: "password12",
        next: "/club-pilot.html",
      },
      maxRedirects: 0,
      failOnStatusCode: false,
    });
    const loc = res.headers().location ?? "";
    expect(loc).not.toMatch(/[?&]password=/i);
    expect(loc).toContain("mode=password");
  });

  test("defaults UI to Spanish when Accept-Language is English", async ({
    browser,
  }) => {
    const context = await browser.newContext({
      locale: "en-US",
      extraHTTPHeaders: { "Accept-Language": "en-US,en;q=0.9" },
      storageState: { cookies: [], origins: [] },
    });
    const page = await context.newPage();
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    const lang = await page.evaluate(() => document.documentElement.lang);
    expect(lang).toMatch(/^es/i);
    await expect(page.locator("h1")).toContainText(/iniciar sesión/i);
    await expect(page.locator("#magic-btn")).toContainText(/enlace/i);
    await context.close();
  });

  test("serves Rust HTML_CSP on /login", async ({ page }) => {
    const res = await page.goto("/login", { waitUntil: "commit" });
    const csp = res?.headers()["content-security-policy"];
    expect(csp?.trim()).toBe(HTML_PAGE_CSP);
  });

  test("no inline scripts on /login", async ({ page }) => {
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    const inline = await page.locator("script:not([src])").count();
    expect(inline).toBe(0);
  });

  test("mode=password reveals POST form without password in page URL", async ({
    page,
  }) => {
    const tracker = trackPasswordInRequestUrls(page);
    await page.goto("/login?mode=password&next=%2Fclub-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    assertNoPasswordInUrl(page.url());
    const form = page.locator("#password-form");
    await expect(form).toBeVisible();
    await expect(form).toHaveAttribute("method", "post");
    await expect(form).toHaveAttribute("action", "/api/account/login");
    tracker.assertClean();
  });
});