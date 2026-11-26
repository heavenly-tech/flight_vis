import { expect, test, type Page } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" },
});

async function loginAccount(page: Page, email: string, password = "password12"): Promise<void> {
  const res = await page.request.post("/api/account/login", {
    headers: { Accept: "application/json" },
    data: { email, password },
  });
  expect(res.ok()).toBeTruthy();
}

test("Mis clubes auto-activates sole active club, hides Continuar without next, shows email/role/badge, and Administrar enters admin without slug typing", async ({
  page,
}) => {
  await stubArcgisTiles(page);
  await loginAccount(page, "admin@demo.test");

  // Confirm activeMembershipId starts null right after password login.
  const beforeMe = await (await page.request.get("/api/account/me")).json();
  expect(beforeMe.activeMembershipId).toBeNull();

  const switchBodies: Array<{ membershipId?: string }> = [];
  page.on("request", (req) => {
    if (req.method() === "POST" && req.url().includes("/api/account/switch-club")) {
      try {
        switchBodies.push(req.postDataJSON() as { membershipId?: string });
      } catch {
        /* ignore */
      }
    }
  });

  await page.goto("/account-clubs.html", { waitUntil: "networkidle" });

  // Signed-in email shown at top; manual-slug form and button removed.
  await expect(page.locator("#account-email")).toHaveText("admin@demo.test");
  await expect(page.locator("#admin-panel")).toHaveCount(0);
  await expect(page.locator("#admin-open")).toHaveCount(0);

  // Continuar is hidden without ?next= (Visor global link stays).
  await expect(page.locator("#continue-globe")).toBeHidden();
  await expect(page.locator('.actions a[href="/"]')).toBeVisible();
  await expect(page.locator('.actions a[href="/account.html"]')).toHaveText("Contraseña de cuenta");
  await expect(page.locator("#logout-btn")).toHaveText("Cerrar sesión");

  // Sole active membership was auto-activated and visibly marked with Club actual badge.
  const activeAdminCard = page.locator("li.club-card.is-current").first();
  await expect(activeAdminCard).toBeVisible();
  await expect(activeAdminCard.locator(".club-current-badge")).toHaveText("Club actual");
  await expect(activeAdminCard.locator(".club-card-meta")).toContainText("Administrador");
  await expect(activeAdminCard.locator(".club-card-meta")).toContainText("activo");
  await expect(activeAdminCard).not.toContainText("(activo)");
  expect(switchBodies.length).toBeGreaterThanOrEqual(1);

  // Continuar also stays hidden when ?next=/ (default).
  await page.goto("/account-clubs.html?next=%2F", { waitUntil: "networkidle" });
  await expect(page.locator("#continue-globe")).toBeHidden();

  // Clicking Contraseña de cuenta opens /account.html.
  await Promise.all([
    page.waitForURL(/\/account\.html/),
    page.locator('.actions a[href="/account.html"]').click(),
  ]);
  await expect(page.locator("#email-line")).toHaveText("admin@demo.test");

  // Go back to Mis clubes and click Administrar -> enters /club-admin-map.html without typing a slug.
  await Promise.all([
    page.waitForURL(/\/account-clubs\.html/),
    page.locator('a[href="/account-clubs.html"]').click(),
  ]);
  const adminBtn = page.locator("li.club-card .club-card-admin").first();
  await expect(adminBtn).toHaveText("Administrar");
  await expect(adminBtn).toHaveClass(/\bbtn-primary\b/);

  await Promise.all([
    page.waitForURL(/\/club-admin-map\.html/),
    adminBtn.click(),
  ]);
  await expect(page.locator("#admin-header-club")).toBeVisible();
});

test("Mis clubes shows Continuar for explicit non-default ?next=, handles multi-club pilot/admin actions, and Cerrar sesión logs out", async ({
  page,
}) => {
  await loginAccount(page, "admin@demo.test");

  // Mock multi-club /api/account/me (two active clubs + one pending, none active yet) so we can test
  // no auto-activate, pilot selection, admin switch with membershipId, and Continuar.
  let activeId: string | null = null;
  const switchCalls: string[] = [];
  await page.route("**/api/account/me", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        account: { id: "acct-1", email: "multi@demo.test" },
        activeMembershipId: activeId,
        memberships: [
          { id: "mem-pilot-1", slug: "club-piloto", name: "Club Piloto", role: "pilot", status: "active" },
          { id: "mem-admin-2", slug: "club-admin", name: "Club Admin", role: "admin", status: "active" },
          { id: "mem-pend-3", slug: "club-pend", name: "Club Pendiente", role: "pilot", status: "pending" },
        ],
      }),
    });
  });
  await page.route("**/api/account/switch-club", async (route) => {
    const body = route.request().postDataJSON() as { membershipId: string };
    switchCalls.push(body.membershipId);
    activeId = body.membershipId;
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ok: true }),
    });
  });

  await page.goto("/account-clubs.html?next=%2Fclub-pilot.html", { waitUntil: "networkidle" });

  await expect(page.locator("#account-email")).toHaveText("multi@demo.test");
  await expect(page.locator("#continue-globe")).toBeVisible();
  expect(switchCalls).toEqual([]);
  await expect(page.locator(".club-current-badge")).toHaveCount(0);

  // Pilot card shows Piloto role + Usar este club; clicking activates it and marks Club actual.
  const pilotCard = page.locator('li.club-card[data-membership-id="mem-pilot-1"]');
  await expect(pilotCard.locator(".club-card-meta")).toHaveText("Piloto · activo");
  await pilotCard.locator(".club-card-select").click();
  await expect(pilotCard.locator(".club-current-badge")).toHaveText("Club actual");
  expect(switchCalls).toEqual(["mem-pilot-1"]);

  // Pending card shows status with no action buttons.
  const pendCard = page.locator('li.club-card[data-membership-id="mem-pend-3"]');
  await expect(pendCard.locator(".club-card-meta")).toHaveText("Piloto · pendiente");
  await expect(pendCard.locator("button")).toHaveCount(0);

  // Clicking Continuar when none is active activates first active membership and goes to ?next=.
  activeId = null;
  switchCalls.length = 0;
  await page.reload({ waitUntil: "networkidle" });
  await expect(page.locator("#continue-globe")).toBeVisible();
  await Promise.all([
    page.waitForURL(/\/club-pilot\.html/),
    page.locator("#continue-globe").click(),
  ]);
  expect(switchCalls).toEqual(["mem-pilot-1"]);

  // Return to Mis clubes and test real Cerrar sesión against the backend.
  await page.unroute("**/api/account/me");
  await page.unroute("**/api/account/switch-club");
  await page.goto("/account-clubs.html", { waitUntil: "networkidle" });

  await Promise.all([
    page.waitForURL(/\/login/),
    page.locator("#logout-btn").click(),
  ]);

  const acctAfter = await page.request.get("/api/account/me");
  expect(acctAfter.status()).toBe(401);
});

test("password-mode login shows password lead (SSR and JS), collapsed Android panel, and secondary Registrar piloto", async ({
  page,
  browser,
}) => {
  // 1. Direct SSR load without JS (es-CL).
  const noJsCtx = await browser.newContext({
    javaScriptEnabled: false,
    locale: "es-CL",
    extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" },
  });
  const noJsPage = await noJsCtx.newPage();
  await noJsPage.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
  await expect(noJsPage.locator("#login-lead")).toHaveText("Inicia sesión con tu contraseña.");
  await expect(noJsPage.locator("#login-lead")).not.toContainText("te enviaremos un enlace");
  await expect(noJsPage.locator("#legacy-details")).toBeVisible();
  await expect(noJsPage.locator("#legacy-details")).not.toHaveAttribute("open", /.*/);
  await expect(noJsPage.locator("#legacy-submit")).toBeHidden();
  await expect(noJsPage.locator("#register-pilot")).toHaveClass(/\bsecondary\b/);
  await expect(noJsPage.locator("#register-pilot")).not.toHaveClass(/\bprimary\b/);
  await noJsCtx.close();

  // 2. Direct SSR load without JS (en-US) also localizes password lead with no flash.
  const enNoJsCtx = await browser.newContext({
    javaScriptEnabled: false,
    locale: "en-US",
    extraHTTPHeaders: { "Accept-Language": "en-US,en;q=0.9" },
  });
  const enNoJsPage = await enNoJsCtx.newPage();
  await enNoJsPage.goto("/login?mode=password", { waitUntil: "domcontentloaded" });
  await expect(enNoJsPage.locator("#login-lead")).toHaveText("Sign in with your password.");
  await expect(enNoJsPage.locator("#legacy-details")).toBeVisible();
  await expect(enNoJsPage.locator("#legacy-details")).not.toHaveAttribute("open", /.*/);
  await enNoJsCtx.close();

  // 3. Interactive toggle on /login with JS: magic -> password -> magic.
  await page.goto("/login", { waitUntil: "networkidle" });
  await expect(page.locator("#login-lead")).toHaveText(
    "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.",
  );
  await expect(page.locator("#legacy-details")).toBeHidden();
  await expect(page.locator("#register-pilot")).toHaveClass(/\bsecondary\b/);
  await expect(page.locator("#register-pilot")).not.toHaveClass(/\bprimary\b/);

  await page.locator("#use-password").click();
  await expect(page.locator("#login-lead")).toHaveText("Inicia sesión con tu contraseña.");
  await expect(page.locator("#legacy-details")).toBeVisible();
  await expect(page.locator("#legacy-details")).not.toHaveAttribute("open", /.*/);
  await expect(page.locator("#legacy-submit")).toBeHidden();

  // Expanding summary reveals the legacy submit button.
  await page.locator("#legacy-details summary").click();
  await expect(page.locator("#legacy-submit")).toBeVisible();

  await page.locator("#back-magic").click();
  await expect(page.locator("#login-lead")).toHaveText(
    "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.",
  );
  await expect(page.locator("#legacy-details")).toBeHidden();
});
