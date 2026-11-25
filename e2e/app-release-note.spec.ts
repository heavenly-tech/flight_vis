import { test, expect } from "@playwright/test";

test("app-release.json message is shown as release note", async ({ page, request }) => {
  // Route app-release.json with a message
  await page.route(
    "**/app/app-release.json",
    (route) => {
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          available: true,
          version: "1.1.0",
          sha256: null,
          apkUrl: "/app/flightvis.apk",
          message: "Desinstala la app anterior antes de instalar esta versi\u00f3n.",
        }),
      });
    },
  );

  // Route HEAD request for apk
  await page.route("/app/flightvis.apk", (route) => {
    if (route.request().method() === "HEAD") {
      route.fulfill({ status: 200 });
    } else {
      route.continue();
    }
  });

  await page.goto("/app.html", { waitUntil: "domcontentloaded" });

  // Wait for the release note to be visible
  await expect(page.locator("#release-note")).toBeVisible({ timeout: 15_000 });
  await expect(page.locator("#release-note")).toHaveText(
    "Desinstala la app anterior antes de instalar esta versi\u00f3n.",
  );
});

test("app-release.json message with HTML is rendered literally", async ({ page, request }) => {
  // Route app-release.json with a message containing HTML
  await page.route(
    "**/app/app-release.json",
    (route) => {
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          available: true,
          version: "1.1.0",
          sha256: null,
          apkUrl: "/app/flightvis.apk",
          message: "<b>x</b>",
        }),
      });
    },
  );

  // Route HEAD request for apk
  await page.route("/app/flightvis.apk", (route) => {
    if (route.request().method() === "HEAD") {
      route.fulfill({ status: 200 });
    } else {
      route.continue();
    }
  });

  await page.goto("/app.html", { waitUntil: "domcontentloaded" });

  // Wait for the release note to be visible
  await expect(page.locator("#release-note")).toBeVisible({ timeout: 15_000 });
  
  // Check that the content is literal, not HTML
  const content = await page.locator("#release-note").textContent();
  expect(content).toBe("<b>x</b>");
  
  // Check that there's no <b> element inside
  await expect(page.locator("#release-note b")).toHaveCount(0);
});

test("app-release.json markdown message renders safely once without duplicating in status", async ({
  page,
}) => {
  await page.route("**/app/app-release.json", (route) => {
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        available: false,
        version: "1.2.0",
        sha256: null,
        apkUrl: "/app/flightvis.apk",
        message: "**Importante:** desinstala la app anterior y mantén <b>literal</b>.",
      }),
    });
  });

  await page.route("/app/flightvis.apk", (route) => {
    if (route.request().method() === "HEAD") {
      route.fulfill({ status: 404 });
    } else {
      route.continue();
    }
  });

  await page.goto("/app.html", { waitUntil: "domcontentloaded" });

  const releaseNote = page.locator("#release-note");
  await expect(releaseNote).toBeVisible({ timeout: 15_000 });
  await expect(releaseNote.locator("strong")).toHaveText("Importante:");
  await expect(releaseNote.locator("b")).toHaveCount(0);
  await expect(releaseNote).toContainText("<b>literal</b>");
  expect(await releaseNote.textContent()).not.toContain("**");
  // Message is rendered once in #release-note and not duplicated in #status
  await expect(page.locator("#status")).not.toContainText("Importante:");
});
