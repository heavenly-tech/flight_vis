import { test, expect } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

test.describe("club login under prod CSP", () => {
  test.setTimeout(60_000);

  test("JS login never puts password in the URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    await page.goto("/club-login.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(() => window.__fvClubLoginReady === true);
    await page.fill('input[name="email"]', "admin@demo.test");
    await page.fill('input[name="password"]', "password12");
    await Promise.all([
      page.waitForURL(/club-pilot\.html/),
      page.locator("#form button[type=submit]").click(),
    ]);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual(
      [],
    );
  });
});