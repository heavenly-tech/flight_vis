import { test, expect } from "@playwright/test";
import { assertNoPasswordInUrl } from "./club-login-helpers";

async function loginPilotViaClubApi(
  page: import("@playwright/test").Page,
): Promise<void> {
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
  expect(res.status()).toBe(303);
  const loc = res.headers().location ?? "";
  assertNoPasswordInUrl(loc);
  await page.goto("/club-pilot.html", { waitUntil: "domcontentloaded" });
}

test("club pilot session loads profile after API login", async ({ page }) => {
  await loginPilotViaClubApi(page);
  await expect(page).toHaveURL(/club-pilot\.html/);
  const me = await page.request.get("/api/club/i18n/context");
  expect(me.ok()).toBeTruthy();
  const json = (await me.json()) as { userLocale?: string | null };
  expect(json.userLocale).toBeTruthy();
});

test("logged-in pilot locale persists across reload", async ({ page }) => {
  await loginPilotViaClubApi(page);
  const patch = await page.request.patch("/api/club/me/locale", {
    data: { locale: "en" },
  });
  expect(patch.ok()).toBeTruthy();
  await page.reload({ waitUntil: "domcontentloaded" });
  const ctx1 = await page.request.get("/api/club/i18n/context");
  expect((await ctx1.json() as { userLocale?: string }).userLocale).toBe("en");
  await page.reload({ waitUntil: "domcontentloaded" });
  const ctx2 = await page.request.get("/api/club/i18n/context");
  expect((await ctx2.json() as { userLocale?: string }).userLocale).toBe("en");
});