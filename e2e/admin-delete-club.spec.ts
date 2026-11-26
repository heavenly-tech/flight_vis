import fs from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test.describe("admin club settings danger zone (delete club)", () => {
  test.setTimeout(60_000);

  test("admin sees danger zone, must type exact slug, submits mocked DELETE, and navigates to redirect", async ({
    page,
  }) => {
    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();

    await stubArcgisTiles(page);
    await page.route("**/api/admin/map", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ aircraft: [] }),
      });
    });

    // Always intercept DELETE /api/admin/club so the shared demo club is never deleted.
    const deletePayloads: unknown[] = [];
    await page.route("**/api/admin/club", async (route) => {
      if (route.request().method() === "DELETE") {
        deletePayloads.push(route.request().postDataJSON());
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ ok: true, redirect: "/account-clubs.html" }),
        });
        return;
      }
      await route.continue();
    });

    await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });

    const settings = page.locator("#club-settings");
    await expect(settings).toBeVisible();
    await expect(settings).not.toHaveAttribute("open", "");

    // Open club settings drawer.
    await page.locator("#club-settings > summary").click();
    await expect(settings).toHaveAttribute("open", "");

    const dangerZone = page.locator("#club-danger-zone");
    await expect(dangerZone).toBeVisible();
    await expect(dangerZone.locator("h2")).toHaveText("Eliminar club");
    await expect(page.locator("#club-delete-slug-expected")).toHaveText("demo");

    const confirmInput = page.locator("#club-delete-slug-input");
    const confirmBtn = page.locator("#club-delete-confirm");
    await expect(confirmBtn).toHaveText("Eliminar definitivamente");
    await expect(confirmBtn).toBeDisabled();

    // Capture screenshot of the open danger zone when requested.
    const shotPath = process.env.DELETE_CLUB_SHOT_PATH;
    if (shotPath) {
      fs.mkdirSync(path.dirname(shotPath), { recursive: true });
      await dangerZone.scrollIntoViewIfNeeded();
      await page.screenshot({ path: shotPath, fullPage: true });
    }

    // Wrong slug and wrong casing keep button disabled.
    await confirmInput.fill("otro-club");
    await expect(confirmBtn).toBeDisabled();

    await confirmInput.fill("DEMO");
    await expect(confirmBtn).toBeDisabled();

    // Exact current slug enables button.
    await confirmInput.fill("demo");
    await expect(confirmBtn).toBeEnabled();

    await Promise.all([
      page.waitForURL(/\/account-clubs\.html$/),
      confirmBtn.click(),
    ]);

    expect(deletePayloads).toEqual([{ slug: "demo" }]);
  });

  test("staff user does not see the danger zone in club settings", async ({ page }) => {
    const login = await page.request.post("/api/club/login", {
      data: { email: "admin@demo.test", password: "password12" },
    });
    expect(login.ok()).toBeTruthy();

    await stubArcgisTiles(page);
    await page.route("**/api/admin/map", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ aircraft: [] }),
      });
    });
    await page.route("**/api/club/me", async (route) => {
      const res = await route.fetch();
      const json = (await res.json()) as {
        pilot?: Record<string, unknown> | null;
        [k: string]: unknown;
      };
      if (json.pilot) {
        json.pilot = { ...json.pilot, role: "staff" };
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(json),
      });
    });
    await page.route("**/api/admin/club", async (route) => {
      if (route.request().method() === "DELETE") {
        await route.fulfill({
          status: 403,
          contentType: "application/json",
          body: JSON.stringify({ error: "admin only" }),
        });
        return;
      }
      await route.continue();
    });

    await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });
    await page.locator("#club-settings > summary").click();
    await expect(page.locator("#club-settings")).toHaveAttribute("open", "");
    await expect(page.locator("#club-danger-zone")).toBeHidden();
  });
});
