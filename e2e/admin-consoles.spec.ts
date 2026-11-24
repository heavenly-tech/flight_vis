import { expect, test } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es" },
});

test("Solicitudes: nav badge matches page count and reject uses in-page dialog", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  let requests = [
    {
      id: "req-e2e-1",
      pilotName: "Diego Rojas",
      pilotEmail: "diego@club.cl",
      requestedTail: "CC-AAA",
      typeCode: "C152",
      newTypeName: null,
      status: "pending",
    },
    {
      id: "req-e2e-2",
      pilotName: "Sofía Díaz",
      pilotEmail: "sofia@club.cl",
      requestedTail: "CC-BBB",
      typeCode: "DA20",
      newTypeName: null,
      status: "pending",
    },
  ];
  const rejectedPayloads: Array<{ id: string; reason: string }> = [];

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
      body: JSON.stringify({ pilots: 4, accessRequests: requests.length }),
    });
  });
  await page.route("**/api/admin/access-requests?status=pending", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ requests }),
    });
  });
  await page.route("**/api/admin/access-requests/*/reject", async (route) => {
    const id = decodeURIComponent(route.request().url().split("/").slice(-2)[0] ?? "");
    const body = (route.request().postDataJSON() ?? {}) as { reason?: string };
    rejectedPayloads.push({ id, reason: body.reason ?? "" });
    requests = requests.filter((r) => r.id !== id);
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ok: true }),
    });
  });

  await page.goto("/club-admin-requests.html", { waitUntil: "networkidle" });

  await expect(page.locator("h1")).toContainText("Solicitudes de acceso");
  await expect(page.locator("#list article")).toHaveCount(2);
  await expect(page.locator("#badge")).toHaveText("2");
  await expect(page.locator("#admin-nav-requests-badge")).toHaveText("2");

  // Open reject dialog -> cancel keeps card.
  const firstCard = page.locator('article[data-request-id="req-e2e-1"]');
  await firstCard.locator(".btn-reject").click();
  const dialog = page.locator("#reject-dialog");
  await expect(dialog).toBeVisible();
  await page.locator("#reject-cancel").click();
  await expect(dialog).toBeHidden();
  expect(rejectedPayloads).toEqual([]);
  await expect(page.locator("#list article")).toHaveCount(2);

  // Reject with reason -> removes card and updates both badges to 1.
  await firstCard.locator(".btn-reject").click();
  await page.locator("#reject-reason").fill("Requiere vuelo con instructor");
  await page.locator("#reject-confirm").click();
  await expect(dialog).toBeHidden();
  await expect(page.locator("#list article")).toHaveCount(1);
  await expect(page.locator("#badge")).toHaveText("1");
  await expect(page.locator("#admin-nav-requests-badge")).toHaveText("1");
  expect(rejectedPayloads).toEqual([
    { id: "req-e2e-1", reason: "Requiere vuelo con instructor" },
  ]);
});

test("Pilotos: invite error displays as alert and Quitar acceso requires dialog confirmation", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  let memberships = [
    {
      id: "mem-e2e-1",
      name: "Carlos Soto",
      email: "carlos@club.cl",
      status: "active",
      role: "pilot",
    },
  ];
  const removedIds: string[] = [];

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
  await page.route("**/api/admin/pilots/pending", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ pilots: [] }),
    });
  });
  await page.route("**/api/admin/memberships", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ memberships }),
    });
  });
  await page.route("**/api/admin/memberships/invite", async (route) => {
    await route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "db error" }),
    });
  });
  await page.route("**/api/admin/memberships/*/remove", async (route) => {
    const id = decodeURIComponent(route.request().url().split("/").slice(-2)[0] ?? "");
    removedIds.push(id);
    memberships = memberships.filter((m) => m.id !== id);
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ok: true }),
    });
  });

  await page.goto("/club-admin-pilots.html", { waitUntil: "networkidle" });

  const row = page.locator('.mem-row[data-membership-id="mem-e2e-1"]');
  await expect(row).toContainText("Piloto · Activo");

  // Invite error renders with role="alert" and error class, never success copy.
  await page.locator("#invite-email").fill("nuevo@club.cl");
  await page.locator('#invite-form button[type="submit"]').click();
  const inviteStatus = page.locator("#invite-status");
  await expect(inviteStatus).toHaveAttribute("role", "alert");
  await expect(inviteStatus).toHaveClass(/status-error/);
  await expect(inviteStatus).not.toContainText("enviamos una invitación");

  // Quitar acceso opens confirmation dialog -> cancel keeps row -> confirm removes it.
  await row.locator(".btn-remove-access").click();
  const dialog = page.locator("#remove-dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Carlos Soto");
  await page.locator("#remove-cancel").click();
  await expect(dialog).toBeHidden();
  expect(removedIds).toEqual([]);
  await expect(row).toHaveCount(1);

  await row.locator(".btn-remove-access").click();
  await expect(dialog).toBeVisible();
  await page.locator("#remove-confirm").click();
  await expect(dialog).toBeHidden();
  await expect(row).toHaveCount(0);
  expect(removedIds).toEqual(["mem-e2e-1"]);
});
