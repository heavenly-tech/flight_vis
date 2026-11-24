import { expect, test } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("Habilitaciones: es-CL labels, Santiago date, type/tail selects, and revoke dialog flow", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  let checkouts = [
    {
      id: "co-e2e-1",
      pilot_id: "pilot-1",
      pilot_name: "Ana Pérez",
      pilot_email: "ana@club.cl",
      type_code: "C152",
      tail: "CC-AAA",
      // 2026-12-31T01:30:00Z -> 30/12/2026 in America/Santiago (UTC-3)
      expires_at: Date.UTC(2026, 11, 31, 1, 30, 0),
    },
    {
      id: "co-e2e-2",
      pilot_id: "pilot-1",
      pilot_name: "Ana Pérez",
      pilot_email: "ana@club.cl",
      type_code: "DA20",
      tail: null,
      expires_at: null,
    },
  ];
  const deletedIds: string[] = [];

  await page.route("**/api/club/i18n/context", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ resolvedLocale: "es" }),
    });
  });
  await page.route("**/api/admin/pending-counts", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ pilots: 0, accessRequests: 0 }),
    });
  });
  await page.route("**/api/admin/pilots", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        pilots: [{ id: "pilot-1", name: "Ana Pérez", email: "ana@club.cl" }],
      }),
    });
  });
  await page.route("**/api/admin/aircraft-types", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        types: [
          { id: "type-1", code: "C152" },
          { id: "type-2", code: "DA20" },
        ],
      }),
    });
  });
  await page.route("**/api/admin/aircraft", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        aircraft: [
          { id: "ac-1", tail: "CC-AAA", type_code: "C152", active: true },
          { id: "ac-2", tail: "CC-BBB", type_code: "DA20", active: true },
        ],
      }),
    });
  });
  await page.route("**/api/admin/checkouts", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ checkouts }),
    });
  });
  await page.route("**/api/admin/checkouts/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const id = decodeURIComponent(route.request().url().split("/").pop() ?? "");
      deletedIds.push(id);
      checkouts = checkouts.filter((c) => c.id !== id);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/club-admin-checkouts.html", { waitUntil: "networkidle" });

  await expect(page.locator("h1")).toHaveText("Habilitaciones");
  await expect(page.locator('label[for="expires_at"]')).toContainText("Vence (opcional)");
  await expect(page.locator("select#type option")).toHaveCount(3);
  await expect(page.locator("select#tail option")).toHaveCount(3);

  const firstRow = page.locator('#list li[data-checkout-id="co-e2e-1"]');
  await expect(firstRow).toContainText("30/12/2026");
  await expect(firstRow).not.toContainText("2026-12-31T");

  // Open revoke confirmation dialog, cancel -> keeps row and sends no DELETE.
  await firstRow.locator(".btn-revoke").click();
  const dialog = page.locator("#revoke-dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Ana Pérez");
  await page.locator("#revoke-cancel").click();
  await expect(dialog).toBeHidden();
  expect(deletedIds).toEqual([]);
  await expect(page.locator("#list li")).toHaveCount(2);

  // Open revoke dialog again, confirm -> sends DELETE and removes row.
  await firstRow.locator(".btn-revoke").click();
  await expect(dialog).toBeVisible();
  await page.locator("#revoke-confirm").click();
  await expect(dialog).toBeHidden();
  await expect(page.locator("#list li")).toHaveCount(1);
  await expect(firstRow).toHaveCount(0);
  expect(deletedIds).toEqual(["co-e2e-1"]);
});
