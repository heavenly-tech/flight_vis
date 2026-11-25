import { expect, test } from "@playwright/test";
import { stubArcgisTiles } from "./globe-csp-helpers";

test.use({ locale: "es-CL", extraHTTPHeaders: { "Accept-Language": "es-CL,es;q=0.9" } });

test("globe HUD, ARIA, and document lang localize in es-CL and switch to en", async ({ page }) => {
  await stubArcgisTiles(page);
  await page.goto("/");

  await expect(page.locator("#flight-name")).not.toHaveText(/Loading|Cargando/, { timeout: 15_000 });
  await expect(page.locator("#hud")).toBeVisible({ timeout: 15_000 });

  expect(await page.evaluate(() => document.documentElement.lang)).toBe("es-CL");
  await expect(page.locator('[data-i18n="globe.hudSpeed"]')).toHaveText("VEL");
  await expect(page.locator('[data-i18n="globe.hudTime"]')).toHaveText("TIEMPO");
  await expect(page.locator("#summary")).toContainText("máx.");
  await expect(page.locator(".brand-lockup")).toHaveAttribute("aria-label", "Inicio de flight_vis");
  await expect(page.locator("#profile")).toHaveAttribute(
    "aria-label",
    "Perfil de altitud y recorte",
  );

  await page.locator("#btn-more").click();
  await page.locator("#btn-lang").click();

  await expect.poll(() => page.evaluate(() => document.documentElement.lang)).toBe("en");
  await expect(page.locator('[data-i18n="globe.hudSpeed"]')).toHaveText("SPD");
  await expect(page.locator('[data-i18n="globe.hudTime"]')).toHaveText("TIME");
  await expect(page.locator("#summary")).toContainText("max");
  await expect(page.locator(".brand-lockup")).toHaveAttribute("aria-label", "flight_vis home");
  await expect(page.locator("#profile")).toHaveAttribute("aria-label", "Altitude profile and trim");
});

test("auth and signup pages share dark card styling, explicit labels, and clean login lang spacing", async ({
  page,
}) => {
  await page.goto("/login", { waitUntil: "domcontentloaded" });
  const langBox = (await page.locator("#lang-slot").boundingBox())!;
  const h1Box = (await page.locator("h1").boundingBox())!;
  expect(langBox).not.toBeNull();
  expect(h1Box).not.toBeNull();
  expect(h1Box.y).toBeGreaterThanOrEqual(langBox.y + langBox.height + 6);

  for (const route of ["/signup", "/c/demo/registro", "/club-signup.html"]) {
    await page.goto(route, { waitUntil: "domcontentloaded" });
    await expect(page.locator("main.auth-card")).toBeVisible({ timeout: 15_000 });
    const bg = await page
      .locator("main.auth-card")
      .evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg).toBe("rgb(16, 21, 29)");

    const linkColor = await page
      .locator("main a")
      .first()
      .evaluate((el) => getComputedStyle(el).color);
    expect(linkColor).toBe("rgb(103, 232, 249)");

    const unlabeledCount = await page.evaluate(() => {
      const els = [...document.querySelectorAll("input, select, textarea")];
      return els.filter((el) => {
        const t = (el.getAttribute("type") ?? "").toLowerCase();
        if (["hidden", "submit", "button"].includes(t)) return false;
        if (el.getAttribute("aria-hidden") === "true") return false;
        const id = el.getAttribute("id");
        const hasFor = Boolean(id && document.querySelector(`label[for="${id}"]`));
        return !hasFor && !el.closest("label") && !el.getAttribute("aria-label");
      }).length;
    });
    expect(unlabeledCount, `unlabeled controls on ${route}`).toBe(0);
  }
});

test("kiosk layout merges save into one Guardar button and supports keyboard position edits", async ({
  page,
}) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  // Verify the real backend endpoint accepts visibility updates without leaving the demo club hidden.
  const realVis = await page.request.post("/api/admin/club/kiosk-checkin-qr-visible", {
    data: { visible: true },
  });
  expect(realVis.ok()).toBeTruthy();

  // Isolate per-test layout state so parallel --repeat-each workers never race on the demo club row.
  let storedLayout = { x: 0.76, y: 0.76, size: 0.22, rev: 1, checkinQrVisible: true };
  let patchCalls = 0;
  let visibleCalls: boolean[] = [];

  await page.route("**/api/admin/kiosk-layout", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(storedLayout),
      });
      return;
    }
    if (route.request().method() === "PATCH") {
      patchCalls += 1;
      const body = route.request().postDataJSON() as { x: number; y: number; size: number };
      storedLayout = {
        ...storedLayout,
        x: body.x,
        y: body.y,
        size: body.size,
        rev: storedLayout.rev + 1,
      };
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(storedLayout),
      });
      return;
    }
    await route.continue();
  });

  await page.route("**/api/admin/club/kiosk-checkin-qr-visible", async (route) => {
    const body = route.request().postDataJSON() as { visible: boolean };
    visibleCalls.push(body.visible);
    storedLayout = {
      ...storedLayout,
      checkinQrVisible: body.visible,
      rev: storedLayout.rev + 1,
    };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(storedLayout),
    });
  });

  await page.goto("/club-admin-kiosk-layout.html", { waitUntil: "networkidle" });
  await expect(page.locator("body")).not.toContainText("Arrastre");
  await expect(page.locator("#btn-save-visible")).toHaveCount(0);
  await expect(page.locator("#btn-save")).toBeVisible();

  const qrX = page.locator("#qr-x");
  const visibleCheckbox = page.locator("#checkin-qr-visible");
  await expect(qrX).toBeVisible();
  await expect(visibleCheckbox).toBeChecked();

  await qrX.fill("20");
  await qrX.dispatchEvent("change");
  await visibleCheckbox.setChecked(false);
  await expect(page.locator("#preview-qr")).toBeHidden();
  expect(patchCalls).toBe(0);
  expect(visibleCalls).toEqual([]);

  await page.locator("#btn-save").click();
  await expect(page.locator("#save-status")).toHaveText(/^(Guardado|Saved)$/, { timeout: 10_000 });
  expect(patchCalls).toBe(1);
  expect(visibleCalls).toEqual([false]);

  await page.reload({ waitUntil: "networkidle" });
  await expect(visibleCheckbox).not.toBeChecked();
  expect(await qrX.inputValue()).toBe("20");

  await visibleCheckbox.setChecked(true);
  await page.locator("#btn-save").click();
  await expect(page.locator("#save-status")).toHaveText(/^(Guardado|Saved)$/, { timeout: 10_000 });
  expect(visibleCalls).toEqual([false, true]);
});

test("admin map interpolates club slug in language help without double braces", async ({ page }) => {
  const login = await page.request.post("/api/club/login", {
    data: { email: "admin@demo.test", password: "password12" },
  });
  expect(login.ok()).toBeTruthy();

  await page.goto("/club-admin-map.html", { waitUntil: "networkidle" });
  const helpText = await page.locator("#club-locale-help").textContent();
  expect(helpText).toContain("/c/demo");
  expect(helpText).not.toContain("{{");
  expect(helpText).not.toContain("{slug}");
});
