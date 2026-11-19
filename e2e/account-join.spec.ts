import { expect, test } from "@playwright/test";
import { installCspObservers } from "./globe-csp-helpers";

const JOIN_SLUG = "e2e-join";

test.describe("account join club", () => {
  test.setTimeout(90_000);

  test("pending join via slug field does not leak secrets in URL", async ({ page }) => {
    const { consoleErrors } = installCspObservers(page);
    const login = await page.request.post("/api/account/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();
    const joinApi = await page.request.post("/api/account/join", {
      data: { slug: JOIN_SLUG, name: "E2E Joiner" },
    });
    expect(joinApi.status()).toBe(201);
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
    const joinForm = page.locator("#join-form");
    if (!(await joinForm.isHidden())) {
      await joinForm.locator('input[name="slug"]').fill(JOIN_SLUG);
      await joinForm.locator("button[type=submit]").click();
      await page.waitForTimeout(500);
    }
    expect(page.url()).not.toMatch(/password/i);
    expect(consoleErrors.filter((e) => /Content Security Policy/i.test(e))).toEqual([]);
  });
});