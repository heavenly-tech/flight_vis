import { test, expect } from "@playwright/test";

test("app install-link exposes intent with APK browser fallback when configured", async ({
  request,
  page,
}) => {
  const linkRes = await request.get("/api/app/install-link");
  expect(linkRes.ok()).toBeTruthy();
  const link = (await linkRes.json()) as {
    httpsUrl?: string;
    apkUrl?: string;
    intentUrl?: string | null;
  };
  expect(link.httpsUrl).toContain("/app.html");
  expect(link.apkUrl).toMatch(/flightvis\.apk/);

  await page.goto("/app.html", { waitUntil: "domcontentloaded" });
  if (link.intentUrl) {
    expect(link.intentUrl).toContain("S.browser_fallback_url=");
    await expect(page.locator("#open-in-app")).toBeVisible({ timeout: 15_000 });
    const href = await page.locator("#open-in-app").getAttribute("href");
    expect(href).toBe(link.intentUrl);
    expect(href).toContain("S.browser_fallback_url=");
  } else {
    await expect(page.locator("#open-in-app")).toBeHidden();
  }
});

test("assetlinks.json is 404 or application/json when configured", async ({ request }) => {
  const res = await request.get("/.well-known/assetlinks.json");
  if (res.status() === 404) {
    return;
  }
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toContain("application/json");
  const body = await res.json();
  expect(Array.isArray(body)).toBeTruthy();
  if (body.length > 0) {
    expect(body[0].target?.sha256_cert_fingerprints?.length).toBeGreaterThan(0);
  }
});