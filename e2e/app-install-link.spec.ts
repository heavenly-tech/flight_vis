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
    expect(link.intentUrl).toMatch(
      /^intent:#Intent;action=android\.intent\.action\.MAIN;category=android\.intent\.category\.LAUNCHER;/,
    );
    expect(link.intentUrl).toContain("S.browser_fallback_url=");
    await expect(page.locator("#open-in-app")).toBeVisible({ timeout: 15_000 });
    const href = await page.locator("#open-in-app").getAttribute("href");
    expect(href).toBe(link.intentUrl);
  } else {
    await expect(page.locator("#open-in-app")).toBeHidden();
  }
});

test.describe("app page and QR landing (es-CL)", () => {
  test.use({
    locale: "es-CL",
    extraHTTPHeaders: { "Accept-Language": "es" },
  });

  test("app page shows uninstall notice, 4 rendered markdown steps, and QR landing links to /app.html", async ({
    page,
  }) => {
    await page.goto("/app.html", { waitUntil: "domcontentloaded" });
    await expect(page.locator("#uninstall-notice")).toBeVisible();
    await expect(page.locator("#uninstall-notice strong")).toBeVisible();

    const steps = page.locator("#sideload-steps li");
    await expect(steps).toHaveCount(4);
    const stepsText = (await steps.allTextContents()).join(" ");
    expect(stepsText).not.toContain("**");
    expect(stepsText.toLowerCase()).not.toContain("cuando esté disponible");
    expect(stepsText.toLowerCase()).not.toMatch(/\bfixes\b/);
    expect(stepsText).toMatch(/Descargas|Archivos/);
    expect(await page.locator("#sideload-steps strong").count()).toBeGreaterThanOrEqual(4);

    await page.goto("/c/demo", { waitUntil: "domcontentloaded" });
    const guideLink = page.locator("#link-app-install");
    await expect(guideLink).toBeVisible();
    await expect(guideLink).toHaveAttribute("href", "/app.html");
  });
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