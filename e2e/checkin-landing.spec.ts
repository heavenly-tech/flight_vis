import { test, expect } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("checkin expired page for invalid token", async ({ page }) => {
  await page.goto("/c/demo/q/invalid-token-value");
  await expect(page.locator("h1")).toContainText(/no válido/i, {
    timeout: 15_000,
  });
});

test("checkin landing page shows app version and release note", async ({ page }) => {
  test.setTimeout(60_000);
  const releaseData = {
    available: true,
    version: "1.1.0+2",
    sha256: null,
    apkUrl: "/app/flightvis.apk",
    message: "Desinstala la app anterior antes de instalar esta versión.",
  };
  await page.route("**/app/app-release.json", (route) => {
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(releaseData),
    });
  });
  await page.goto("/c/demo");
  await expect(page.locator("#app-version")).toContainText("1.1.0+2", {
    timeout: 15_000,
  });
  await expect(page.locator("#app-release-note")).toHaveText(
    "Desinstala la app anterior antes de instalar esta versión.",
  );
  await expect(page.locator("#btn-download-primary")).toBeVisible();
});
