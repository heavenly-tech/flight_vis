import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

test.describe("account join club", () => {
  test.setTimeout(90_000);

  test("pending join via slug field does not leak secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    const login = await page.request.post("/api/account/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();
    await page.goto("/account-clubs.html", { waitUntil: "domcontentloaded" });
    await page.waitForFunction(
      async () => {
        const res = await fetch("/api/account/me", { credentials: "same-origin" });
        if (!res.ok) return false;
        const data = await res.json();
        return Array.isArray(data.memberships) && data.memberships.length > 0;
      },
      undefined,
      { timeout: 25_000 },
    );
    await page.locator("#join-open").click();
    await expect(page.locator("#join-panel")).toBeVisible({ timeout: 10_000 });
    const joinForm = page.locator("#join-form");
    await expect(joinForm).toBeVisible();
    await joinForm.locator('input[name="slug"]').fill("demo");
    await joinForm.locator("button[type=submit]").click();
    await page.waitForTimeout(800);
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});