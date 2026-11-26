import { expect, test, type Page } from "@playwright/test";

async function loginDemoAdmin(page: Page): Promise<void> {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();
}

async function stubLocale(page: Page, locale: "es" | "en"): Promise<void> {
  await page.route("**/api/club/i18n/context", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ resolvedLocale: locale }),
    });
  });
}

test.describe("Admin data UX (es-CL)", () => {
  test.use({
    locale: "es-CL",
    extraHTTPHeaders: { "Accept-Language": "es" },
  });

  test("Pilotos: invite sends role, renders active rating chips + Habilitar link -> ?pilot= preselect, pending pilots only in Solicitudes", async ({
    page,
  }) => {
    await loginDemoAdmin(page);
    await stubLocale(page, "es");

    const invitePayloads: Array<{ email: string; role: string }> = [];
    let pendingPilots = [
      { id: "pilot-pend-1", name: "Pedro Pendiente", email: "pedro@demo.test" },
    ];
    const memberships = [
      {
        id: "mem-1",
        pilotId: "pilot-ok-1",
        name: "Ana Piloto",
        email: "ana@demo.test",
        role: "pilot",
        status: "active",
      },
      {
        id: "mem-2",
        pilotId: "pilot-ok-2",
        name: "Beto SinHab",
        email: "beto@demo.test",
        role: "pilot",
        status: "active",
      },
      {
        id: "mem-pend-1",
        pilotId: "pilot-pend-1",
        name: "Pedro Pendiente",
        email: "pedro@demo.test",
        role: "pilot",
        status: "pending",
      },
    ];
    const checkouts = [
      {
        id: "co-1",
        pilot_id: "pilot-ok-1",
        pilot_name: "Ana Piloto",
        pilot_email: "ana@demo.test",
        type_code: "C152",
        tail: null,
        expires_at: null,
      },
      {
        id: "co-2",
        pilot_id: "pilot-ok-1",
        pilot_name: "Ana Piloto",
        pilot_email: "ana@demo.test",
        type_code: "DA20",
        tail: "CC-ABC",
        expires_at: Date.now() + 86_400_000,
      },
    ];
    const pilots = [
      { id: "pilot-ok-1", name: "Ana Piloto", email: "ana@demo.test", status: "approved" },
      { id: "pilot-ok-2", name: "Beto SinHab", email: "beto@demo.test", status: "approved" },
      { id: "pilot-pend-1", name: "Pedro Pendiente", email: "pedro@demo.test", status: "pending" },
    ];

    await page.route("**/api/admin/pending-counts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: pendingPilots.length, accessRequests: 0 }),
      });
    });
    await page.route("**/api/admin/memberships", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ memberships }),
      });
    });
    await page.route("**/api/admin/pilots/pending", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: pendingPilots }),
      });
    });
    await page.route("**/api/admin/pilots", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots }),
      });
    });
    await page.route("**/api/admin/pilots/*/approve", async (route) => {
      const id = decodeURIComponent(route.request().url().split("/").slice(-2)[0] ?? "");
      pendingPilots = pendingPilots.filter((p) => p.id !== id);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
    });
    await page.route("**/api/admin/access-requests?status=pending", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ requests: [] }),
      });
    });
    await page.route("**/api/admin/aircraft-types", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ types: [{ id: "t-1", code: "C152" }] }),
      });
    });
    await page.route("**/api/admin/aircraft", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          aircraft: [{ id: "a-1", tail: "CC-ABC", type_code: "C152", active: true }],
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
    await page.route("**/api/admin/memberships/invite", async (route) => {
      const body = (route.request().postDataJSON() ?? {}) as { email: string; role: string };
      invitePayloads.push(body);
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ok: true }),
      });
    });

    await page.goto("/club-admin-pilots.html", { waitUntil: "networkidle" });

    // Updated nav labels (Item 17a) and Solicitudes badge includes pending pilot; Pilotos badge hidden.
    await expect(page.locator('a[href="/club-admin-kiosk-layout.html"]')).toHaveText(
      "Diseño de pantalla",
    );
    await expect(page.locator('a[href="/club-admin-checkin-qr.html"]')).toHaveText(
      "QR en celular",
    );
    await expect(page.locator("#admin-nav-requests-badge")).toHaveText("1");
    await expect(page.locator("#admin-nav-pilots-badge")).toBeHidden();

    // No "Pilotos pendientes" list on Pilotos; pending membership excluded; notice links to Solicitudes.
    await expect(page.locator("#pending")).toHaveCount(0);
    await expect(page.locator("#memberships .mem-row")).toHaveCount(2);
    await expect(page.locator("#memberships")).not.toContainText("Pedro Pendiente");
    const notice = page.locator("#pending-pilots-notice");
    await expect(notice).toBeVisible();
    await expect(notice).toContainText("1 piloto espera aprobación en Solicitudes.");
    await expect(notice.locator('a[href="/club-admin-requests.html"]')).toBeVisible();

    // Invite card sends { email, role: "pilot" } by default and { email, role: "admin" } when selected.
    await expect(page.locator("#invite-section h2")).toHaveText("Invitar al club");
    await expect(page.locator("#invite-role")).toHaveValue("pilot");
    await page.locator("#invite-email").fill("nuevo-piloto@demo.test");
    await page.locator('#invite-form button[type="submit"]').click();
    await expect(page.locator("#invite-status")).toHaveText(
      "Si el correo es válido, enviamos una invitación de piloto.",
    );

    await page.locator("#invite-role").selectOption("admin");
    await page.locator("#invite-email").fill("nuevo-admin@demo.test");
    await page.locator('#invite-form button[type="submit"]').click();
    await expect(page.locator("#invite-status")).toHaveText(
      "Si el correo es válido, enviamos una invitación de administrador.",
    );
    expect(invitePayloads).toEqual([
      { email: "nuevo-piloto@demo.test", role: "pilot" },
      { email: "nuevo-admin@demo.test", role: "admin" },
    ]);

    // Ratings chips & empty state + Habilitar link -> navigates with ?pilot= and preselects.
    const anaRow = page.locator('.mem-row[data-membership-id="mem-1"]');
    await expect(anaRow.locator(".rating-chip")).toHaveText([
      "C152 · Todas las matrículas del tipo",
      "DA20 · CC-ABC",
    ]);
    const betoRow = page.locator('.mem-row[data-membership-id="mem-2"]');
    await expect(betoRow.locator(".rating-chip")).toHaveCount(0);
    await expect(betoRow).toContainText("Sin habilitaciones");

    await anaRow.locator(".rating-link").click();
    await page.waitForURL(/\/club-admin-checkouts\.html\?pilot=pilot-ok-1/);
    await expect(page.locator("#pilot")).toHaveValue("pilot-ok-1");
    await expect(page.locator("#pending-pilot-warning")).toBeHidden();

    // Pending pilot appears in Solicitudes ABOVE checkout requests and can be approved there.
    await page.goto("/club-admin-requests.html", { waitUntil: "networkidle" });
    const pendingBox = (await page.locator("#pending-pilots-section").boundingBox())!;
    const accessBox = (await page.locator("#access-requests-section").boundingBox())!;
    expect(pendingBox.y).toBeLessThan(accessBox.y);
    const pendingRow = page.locator('#pending-pilots li[data-pilot-id="pilot-pend-1"]');
    await expect(pendingRow).toContainText("Pedro Pendiente");
    await expect(page.locator("#badge")).toHaveText("1");
    await pendingRow.locator(".btn-approve").click();
    await expect(page.locator("#pending-pilots")).toContainText("No hay pilotos pendientes.");
    await expect(page.locator("#badge")).toBeHidden();
    await expect(page.locator("#admin-nav-requests-badge")).toBeHidden();
  });

  test("Matrículas: dropdown fed from aircraft types + empty state disables form with link to Tipos", async ({
    page,
  }) => {
    await loginDemoAdmin(page);
    await stubLocale(page, "es");

    let types: Array<{ id: string; code: string }> = [
      { id: "t-1", code: "C152" },
      { id: "t-2", code: "DA20-C1" },
    ];
    let aircraft = [{ id: "a-1", tail: "CC-AAA", type_code: "C152", active: true }];
    const savedBodies: Array<Record<string, unknown>> = [];

    await page.route("**/api/admin/pending-counts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: 0, accessRequests: 0 }),
      });
    });
    await page.route("**/api/admin/aircraft-types", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ types }),
      });
    });
    await page.route("**/api/admin/aircraft", async (route) => {
      if (route.request().method() === "POST") {
        const body = (route.request().postDataJSON() ?? {}) as Record<string, unknown>;
        savedBodies.push(body);
        aircraft = [
          ...aircraft,
          {
            id: "a-2",
            tail: String(body.tail),
            type_code: String(body.type),
            active: Boolean(body.active),
          },
        ];
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ id: "a-2" }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ aircraft }),
      });
    });

    await page.goto("/club-admin-tails.html", { waitUntil: "networkidle" });

    const typeSelect = page.locator("select#type-input");
    const submitBtn = page.locator('#form button[type="submit"]');
    await expect(typeSelect).toBeEnabled();
    await expect(submitBtn).toBeEnabled();
    await expect(typeSelect.locator("option")).toHaveText([
      "Selecciona un tipo",
      "C152",
      "DA20-C1",
    ]);

    await page.locator("#tail-input").fill("CC-BBB");
    await typeSelect.selectOption("DA20-C1");
    await submitBtn.click();
    await expect(page.locator("#list li.admin-row")).toHaveCount(2);
    await expect(typeSelect).toHaveValue("DA20-C1");
    expect(savedBodies).toEqual([{ tail: "CC-BBB", type: "DA20-C1", active: true }]);

    // Empty types -> disables select + submit and shows link to Tipos.
    types = [];
    await page.reload({ waitUntil: "networkidle" });
    await expect(typeSelect).toBeDisabled();
    await expect(submitBtn).toBeDisabled();
    const hint = page.locator("#tails-no-types-hint");
    await expect(hint).toBeVisible();
    await expect(hint).toHaveText("Primero crea un tipo en Tipos.");
    await expect(hint.locator('a[href="/club-admin-types.html"]')).toBeVisible();
  });

  test("Habilitaciones: marks pending pilot, shows role=alert warning with link, keeps 2-column grid on desktop", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await loginDemoAdmin(page);
    await stubLocale(page, "es");

    await page.route("**/api/admin/pending-counts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: 1, accessRequests: 0 }),
      });
    });
    await page.route("**/api/admin/pilots", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          pilots: [
            { id: "p-ok", name: "Ana Aprobada", email: "ana@demo.test", status: "approved" },
            { id: "p-pend", name: "Luis Pendiente", email: "luis@demo.test", status: "pending" },
          ],
        }),
      });
    });
    await page.route("**/api/admin/aircraft-types", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ types: [{ id: "t-1", code: "C152" }] }),
      });
    });
    await page.route("**/api/admin/aircraft", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          aircraft: [{ id: "a-1", tail: "CC-AAA", type_code: "C152", active: true }],
        }),
      });
    });
    await page.route("**/api/admin/checkouts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ checkouts: [] }),
      });
    });

    await page.goto("/club-admin-checkouts.html?pilot=unknown-id", { waitUntil: "networkidle" });

    // Unknown ?pilot= is ignored.
    const pilotSelect = page.locator("#pilot");
    await expect(pilotSelect).toHaveValue("");
    await expect(pilotSelect.locator("option")).toHaveText([
      "Selecciona un piloto",
      "Ana Aprobada (ana@demo.test)",
      "Luis Pendiente (luis@demo.test) · pendiente",
    ]);

    // 2-column grid on desktop: Piloto and Tipo sit on the same row.
    const pilotBox = (await page.locator("#pilot").boundingBox())!;
    const typeBox = (await page.locator("#type").boundingBox())!;
    expect(Math.abs(pilotBox.y - typeBox.y)).toBeLessThanOrEqual(4);
    expect(typeBox.x).toBeGreaterThan(pilotBox.x + pilotBox.width - 1);

    // Selecting pending pilot reveals role=alert warning with link to Solicitudes; submit stays enabled.
    await pilotSelect.selectOption("p-pend");
    const warning = page.locator("#pending-pilot-warning");
    await expect(warning).toBeVisible();
    await expect(warning).toHaveAttribute("role", "alert");
    await expect(warning).toHaveText(
      "Este piloto aún no está aprobado. Apruébalo en Solicitudes antes de habilitarlo.",
    );
    await expect(warning.locator('a[href="/club-admin-requests.html"]')).toBeVisible();
    await expect(page.locator('#form button[type="submit"]')).toBeEnabled();

    // Switching to approved pilot hides warning.
    await pilotSelect.selectOption("p-ok");
    await expect(warning).toBeHidden();
  });

  test("390px mobile viewport has no horizontal overflow on all touched admin pages", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await loginDemoAdmin(page);
    await stubLocale(page, "es");

    await page.route("**/api/admin/pending-counts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: 1, accessRequests: 1 }),
      });
    });
    await page.route("**/api/admin/memberships", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          memberships: [
            {
              id: "mem-1",
              pilotId: "p-1",
              name: "Pilot With Long Name",
              email: "very.long.pilot.address@clubaereosantiago.cl",
              role: "pilot",
              status: "active",
            },
          ],
        }),
      });
    });
    await page.route("**/api/admin/pilots/pending", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          pilots: [{ id: "p-pend", name: "Nuevo Piloto", email: "nuevo.piloto@club.cl" }],
        }),
      });
    });
    await page.route("**/api/admin/pilots", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          pilots: [
            { id: "p-1", name: "Pilot With Long Name", email: "very.long.pilot.address@clubaereosantiago.cl", status: "approved" },
            { id: "p-pend", name: "Nuevo Piloto", email: "nuevo.piloto@club.cl", status: "pending" },
          ],
        }),
      });
    });
    await page.route("**/api/admin/access-requests?status=pending", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          requests: [
            {
              id: "req-1",
              pilotName: "Ana Piloto",
              pilotEmail: "ana@club.cl",
              requestedTail: "CC-AAA",
              typeCode: "C152",
              newTypeName: null,
              status: "pending",
            },
          ],
        }),
      });
    });
    await page.route("**/api/admin/aircraft-types", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ types: [{ id: "t-1", code: "C152" }, { id: "t-2", code: "DA20-C1" }] }),
      });
    });
    await page.route("**/api/admin/aircraft", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          aircraft: [{ id: "a-1", tail: "CC-AAA", type_code: "C152", active: true }],
        }),
      });
    });
    await page.route("**/api/admin/checkouts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          checkouts: [
            {
              id: "co-1",
              pilot_id: "p-1",
              pilot_name: "Pilot With Long Name",
              pilot_email: "very.long.pilot.address@clubaereosantiago.cl",
              type_code: "C152",
              tail: null,
              expires_at: null,
            },
          ],
        }),
      });
    });

    const pages = [
      "/club-admin-pilots.html",
      "/club-admin-requests.html",
      "/club-admin-tails.html",
      "/club-admin-types.html",
      "/club-admin-checkouts.html?pilot=p-pend",
    ];

    for (const route of pages) {
      await page.goto(route, { waitUntil: "networkidle" });
      const overflow = await page.evaluate(() => ({
        scrollWidth: document.documentElement.scrollWidth,
        clientWidth: document.documentElement.clientWidth,
      }));
      expect(overflow.scrollWidth, `horizontal overflow on ${route}`).toBeLessThanOrEqual(
        overflow.clientWidth,
      );
    }

    // On checkouts at 390px, form stacks to 1 column (Tipo sits below Piloto).
    const pilotBox = (await page.locator("#pilot").boundingBox())!;
    const typeBox = (await page.locator("#type").boundingBox())!;
    expect(typeBox.y).toBeGreaterThan(pilotBox.y + pilotBox.height - 1);
  });
});

test.describe("Admin data UX (en locale safety)", () => {
  test.use({
    locale: "en-US",
    extraHTTPHeaders: { "Accept-Language": "en-US,en;q=0.9" },
  });

  test("renders aligned English strings for nav labels, invite, empty types hint, and pending pilot warning", async ({
    page,
  }) => {
    await loginDemoAdmin(page);
    await stubLocale(page, "en");

    await page.route("**/api/admin/pending-counts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ pilots: 1, accessRequests: 0 }),
      });
    });
    await page.route("**/api/admin/memberships", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ memberships: [] }),
      });
    });
    await page.route("**/api/admin/pilots/pending", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          pilots: [{ id: "p-pend", name: "Jane Pending", email: "jane@demo.test" }],
        }),
      });
    });
    await page.route("**/api/admin/pilots", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          pilots: [{ id: "p-pend", name: "Jane Pending", email: "jane@demo.test", status: "pending" }],
        }),
      });
    });
    await page.route("**/api/admin/aircraft-types", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ types: [] }),
      });
    });
    await page.route("**/api/admin/aircraft", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ aircraft: [] }),
      });
    });
    await page.route("**/api/admin/checkouts", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ checkouts: [] }),
      });
    });

    await page.goto("/club-admin-pilots.html", { waitUntil: "networkidle" });
    await expect(page.locator('a[href="/club-admin-kiosk-layout.html"]')).toHaveText("Screen layout");
    await expect(page.locator('a[href="/club-admin-checkin-qr.html"]')).toHaveText("Phone QR");
    await expect(page.locator("#invite-section h2")).toHaveText("Invite to club");
    await expect(page.locator("#pending-pilots-notice")).toHaveText(
      "1 pilot is awaiting approval in Requests.",
    );

    await page.goto("/club-admin-tails.html", { waitUntil: "networkidle" });
    await expect(page.locator("#tails-no-types-hint")).toHaveText("Create a type first in Types.");

    await page.goto("/club-admin-checkouts.html?pilot=p-pend", { waitUntil: "networkidle" });
    await expect(page.locator("#pilot option").nth(1)).toHaveText(
      "Jane Pending (jane@demo.test) · pending",
    );
    await expect(page.locator("#pending-pilot-warning")).toHaveText(
      "This pilot is not approved yet. Approve them in Requests before granting a checkout.",
    );
  });
});
