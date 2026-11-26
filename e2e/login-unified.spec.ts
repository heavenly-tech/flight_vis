import { test, expect } from "@playwright/test";
import {
  assertNoPasswordInUrl,
  trackPasswordInRequestUrls,
} from "./club-login-helpers";
import { HTML_PAGE_CSP } from "./csp-constants";
import {
  expectPasswordRevealed,
  expectSingleFormLayout,
} from "./login-layout-helpers";

test.describe("unified /login", () => {
  test.use({
    locale: "es-CL",
    extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" },
  });

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

  test("defaults UI to English when Accept-Language is English", async ({
    browser,
  }) => {
    const context = await browser.newContext({
      locale: "en-US",
      extraHTTPHeaders: { "Accept-Language": "en-US,en;q=0.9" },
      storageState: { cookies: [], origins: [] },
    });
    const page = await context.newPage();
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await expect(page.locator("h1")).toContainText(/sign in/i, {
      timeout: 15_000,
    });
    const lang = await page.evaluate(() => document.documentElement.lang);
    expect(lang).toMatch(/^en/i);
    await expect(page.locator("h1")).toContainText(/sign in/i);
    await expect(page.locator("#submit-magic")).toContainText(/sign-in link/i);
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

  test("with JS shows one email and login chrome in order", async ({ page }) => {
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await expectSingleFormLayout(page);
    await expect(page.locator("#submit-magic")).toHaveText(/enviar enlace/i);
    await expect(page.locator("#use-password")).toHaveText(/usar contraseña/i);
    await expect(page.locator("#register-pilot")).toHaveText(/registrar piloto/i);
    await expect(page.locator("#register-club")).toHaveText(/registrar club/i);
  });

  test("with JS reveals password in the same form", async ({ page }) => {
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await page.locator("#use-password").click();
    await expectPasswordRevealed(page);
    const form = page.locator("#login-form");
    await expect(form).toHaveAttribute("action", "/api/account/login");
  });

  test("without JS shows Spanish fallbacks and form POST magic", async ({
    browser,
  }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    await page.goto("/login", { waitUntil: "domcontentloaded" });
    await expectSingleFormLayout(page);
    await expect(page.locator("#submit-magic")).toHaveText(/enviar enlace/i);
    await expect(page.locator("#register-pilot")).toHaveText(/registrar piloto/i);
    await expect(page.locator("#login-form")).toHaveAttribute(
      "action",
      "/api/account/magic-link",
    );
    await expect(page.locator("#password-reveal")).toBeHidden();
    await context.close();
  });

  test("without JS on mode=password POSTs form and redirects", async ({
    browser,
  }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    await page.goto("/login?mode=password&next=%2Fclub-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    await expect(page.locator('input[name="email"][type="email"]')).toHaveCount(1);
    await expectPasswordRevealed(page);
    await expect(page.locator("#login-form")).toHaveAttribute(
      "action",
      "/api/account/login",
    );
    await page.locator("#email").fill("nobody@test");
    await page.locator("#password").fill("password1234");
    await Promise.all([
      page.waitForURL((url) => url.searchParams.get("e") === "1", {
        timeout: 30_000,
      }),
      page.locator("#submit-password").click(),
    ]);
    await context.close();
  });

  test("without JS legacy APK formaction POSTs club login", async ({
    browser,
  }) => {
    const context = await browser.newContext({ javaScriptEnabled: false });
    const page = await context.newPage();
    await page.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
    await page.locator("#email").fill("nobody@test");
    await page.locator("#password").fill("password1234");
    const details = page.locator("#legacy-details");
    await expect(details).toBeVisible();
    await expect(details).not.toHaveAttribute("open", /.*/);
    await details.locator("summary").click();
    const legacy = page.locator("#legacy-submit");
    await expect(legacy).toHaveAttribute("formaction", "/api/club/login");
    await Promise.all([
      page.waitForURL((url) => url.searchParams.get("e") === "1", {
        timeout: 30_000,
      }),
      legacy.click(),
    ]);
    expect(page.url()).toContain("e=1");
    await context.close();
  });

  test("mode=password POST form without password in page URL", async ({
    page,
  }) => {
    const tracker = trackPasswordInRequestUrls(page);
    await page.goto("/login?mode=password&next=%2Fclub-pilot.html", {
      waitUntil: "domcontentloaded",
    });
    assertNoPasswordInUrl(page.url());
    const form = page.locator("#login-form");
    await expect(form).toBeVisible();
    await expect(form).toHaveAttribute("method", "post");
    await expect(form).toHaveAttribute("action", "/api/account/login");
    tracker.assertClean();
  });
});