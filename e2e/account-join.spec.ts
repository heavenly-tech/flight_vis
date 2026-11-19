import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

test.describe("account join club", () => {
  test.setTimeout(60_000);

  test("pending join via slug field does not leak secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    const login = await page.request.post("/api/account/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();
    const state = await page.request.storageState();
    await page.context().addCookies(state.cookies);
    await page.goto("/account-clubs.html", { waitUntil: "load" });
    await page.waitForResponse(
      (r) => r.url().includes("/api/account/me") && r.status() === 200,
    );
    await page.locator("#join-open").click();
    const joinForm = page.locator("#join-form");
    await expect(joinForm).toBeVisible();
    await joinForm.locator('input[name="slug"]').fill("demo");
    await joinForm.locator("button[type=submit]").click();
    await page.waitForTimeout(800);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});