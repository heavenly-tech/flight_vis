import { expect, test, type Page } from "@playwright/test";

test.use({
  locale: "es-CL",
  extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" },
});

async function loginAdminWithAccountSession(page: Page): Promise<{ clubName: string; clubSlug: string }> {
  const login = await page.request.post("/api/account/login", {
    headers: { Accept: "application/json" },
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  const meRes = await page.request.get("/api/account/me");
  expect(meRes.ok()).toBeTruthy();
  const me = (await meRes.json()) as {
    memberships: Array<{ id: string; role?: string; status: string }>;
  };
  const adminMem = me.memberships.find((m) => m.status === "active" && m.role === "admin");
  expect(adminMem).toBeTruthy();

  const sw = await page.request.post("/api/account/switch-club", {
    data: { membershipId: adminMem!.id },
  });
  expect(sw.ok()).toBeTruthy();

  const clubMeRes = await page.request.get("/api/club/me");
  expect(clubMeRes.ok()).toBeTruthy();
  const clubMe = (await clubMeRes.json()) as { clubName?: string; clubSlug?: string };
  expect(typeof clubMe.clubName).toBe("string");
  expect(clubMe.clubName!.trim().length).toBeGreaterThan(0);
  return { clubName: clubMe.clubName!.trim(), clubSlug: (clubMe.clubSlug ?? "").trim() };
}

test("admin header shows active club, active tab pill, Mis clubes + Mi cuenta links, no Iniciar sesión, and logout ends session", async ({
  page,
}) => {
  const { clubName } = await loginAdminWithAccountSession(page);

  await page.goto("/club-admin-types.html", { waitUntil: "networkidle" });

  const clubBadge = page.locator("#admin-header-club");
  await expect(clubBadge).toBeVisible();
  await expect(clubBadge).toHaveText(`Club: ${clubName}`);

  const headerBar = page.locator(".admin-header-bar");
  await expect(headerBar.locator('a[href="/account-clubs.html"]')).toHaveText("Mis clubes");
  await expect(headerBar.locator('a[href="/account.html"]')).toHaveText("Mi cuenta");
  await expect(page.locator("#admin-header-logout")).toHaveText("Cerrar sesión");

  // Nav must not contain the old "Iniciar sesión" link.
  const nav = page.locator("nav.admin-nav");
  await expect(nav.locator('a[href*="login"]')).toHaveCount(0);
  await expect(nav).not.toContainText("Iniciar sesión");

  // Active tab has aria-current="page" and solid pill styling.
  const activeTab = nav.locator('a[aria-current="page"]');
  await expect(activeTab).toHaveCount(1);
  await expect(activeTab).toHaveAttribute("href", "/club-admin-types.html");
  const pillColors = await activeTab.evaluate((el) => {
    const cs = getComputedStyle(el);
    return { bg: cs.backgroundColor, fg: cs.color };
  });
  expect(pillColors.bg).toBe("rgb(236, 235, 255)");
  expect(pillColors.fg).toBe("rgb(21, 19, 42)");

  // Navigating to another admin tab updates aria-current="page" and keeps the club header.
  await Promise.all([
    page.waitForURL(/\/club-admin-tails\.html/),
    nav.locator('a[href="/club-admin-tails.html"]').click(),
  ]);
  await expect(page.locator('#admin-nav nav.admin-nav a[aria-current="page"]')).toHaveAttribute(
    "href",
    "/club-admin-tails.html",
  );
  await expect(page.locator("#admin-header-club")).toHaveText(`Club: ${clubName}`);

  // Clicking Cerrar sesión POSTs /api/account/logout, returns to /login, and clears both sessions.
  await Promise.all([
    page.waitForURL(/\/login/),
    page.locator("#admin-header-logout").click(),
  ]);

  const acctAfter = await page.request.get("/api/account/me");
  expect(acctAfter.status()).toBe(401);
  const clubAfter = await page.request.get("/api/club/me");
  expect(clubAfter.ok()).toBeTruthy();
  expect((await clubAfter.json()).pilot).toBeNull();

  // Visiting an admin page after logout redirects to /login.
  await page.goto("/club-admin-types.html", { waitUntil: "load" });
  await expect(page).toHaveURL(/\/login\?/);
  expect(new URL(page.url()).searchParams.get("next")).toBe("/club-admin-types.html");
});

test("Mi cuenta opens /account.html and saving password shows Contraseña guardada.", async ({
  page,
}) => {
  await loginAdminWithAccountSession(page);

  await page.goto("/club-admin-types.html", { waitUntil: "networkidle" });
  await Promise.all([
    page.waitForURL(/\/account\.html/),
    page.locator('.admin-header-bar a[href="/account.html"]').click(),
  ]);

  await expect(page.locator("#email-line")).toHaveText("admin@demo.test");
  await expect(page.locator('a[href="/account-clubs.html"]')).toHaveText("Mis clubes");

  // Intercept password mutation so parallel --repeat-each workers never revoke each other's sessions.
  let postedBody: { current_password?: string; password?: string } | null = null;
  await page.route("**/api/account/password", async (route) => {
    expect(route.request().method()).toBe("POST");
    postedBody = route.request().postDataJSON() as {
      current_password?: string;
      password?: string;
    };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ ok: true }),
    });
  });

  await page.locator("#current").fill("password12");
  await page.locator("#new-password").fill("password12");
  await page.locator('#password-form button[type="submit"]').click();

  const msg = page.locator("#msg");
  await expect(msg).toHaveText("Contraseña guardada.");
  await expect(msg).not.toContainText("Sesión iniciada.");
  expect(postedBody).toEqual({
    current_password: "password12",
    password: "password12",
  });
});

test("at 390px admin nav is a single horizontally scrollable row with no page overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  const { clubName } = await loginAdminWithAccountSession(page);

  await page.goto("/club-admin-types.html", { waitUntil: "networkidle" });
  await expect(page.locator("#admin-header-club")).toHaveText(`Club: ${clubName}`);

  const nav = page.locator("nav.admin-nav");
  await expect(nav).toBeVisible();

  const metrics = await nav.evaluate((el) => {
    const links = [...el.querySelectorAll("a")];
    const linkRects = links.map((a) => {
      const r = a.getBoundingClientRect();
      return { top: r.top, height: r.height };
    });
    const navRect = el.getBoundingClientRect();
    return {
      navHeight: navRect.height,
      scrollWidth: el.scrollWidth,
      clientWidth: el.clientWidth,
      linkCount: links.length,
      linkRects,
      docScrollWidth: document.documentElement.scrollWidth,
      docClientWidth: document.documentElement.clientWidth,
    };
  });

  expect(metrics.linkCount).toBe(8);
  expect(metrics.scrollWidth).toBeGreaterThan(metrics.clientWidth);
  expect(metrics.docScrollWidth).toBeLessThanOrEqual(metrics.docClientWidth);

  const firstTop = metrics.linkRects[0].top;
  const firstHeight = metrics.linkRects[0].height;
  for (const r of metrics.linkRects) {
    expect(Math.abs(r.top - firstTop)).toBeLessThanOrEqual(2);
  }
  expect(metrics.navHeight).toBeLessThanOrEqual(firstHeight + 24);
  expect(metrics.navHeight).toBeLessThanOrEqual(56);
});
