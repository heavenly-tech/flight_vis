import { expect, type Page } from "@playwright/test";

export async function expectSingleFormLayout(page: Page): Promise<void> {
  await expect(page.locator('input[name="email"][type="email"]')).toHaveCount(1);
  await expect(page.locator("#login-form")).toHaveAttribute(
    "action",
    "/api/account/magic-link",
  );
  await expect(page.locator("#submit-magic")).toBeVisible();
  await expect(page.locator("#password-reveal")).toBeHidden();
  await expect(page.locator("#submit-password")).toBeHidden();
  await expect(page.locator("#use-password")).toBeVisible();
  const pilotCta = page.locator("#register-pilot");
  const clubCta = page.locator("#register-club");
  await expect(pilotCta).toBeVisible();
  await expect(clubCta).toBeVisible();
  await expect(pilotCta).toHaveClass(/\bsecondary\b/);
  await expect(pilotCta).not.toHaveClass(/\bprimary\b/);
  await expect(pilotCta).toHaveAttribute("href", "/signup.html");
  await expect(clubCta).toHaveClass(/\bsecondary\b/);
  await expect(clubCta).not.toHaveClass(/\bprimary\b/);
  await expect(clubCta).toHaveAttribute("href", "/club-signup.html");
  const rows = page.locator(
    "#submit-magic, #use-password, #register-pilot, #register-club",
  );
  await expect(rows).toHaveCount(4);
  const order = await rows.evaluateAll((nodes) =>
    nodes.map((n) => n.id || n.getAttribute("href") || ""),
  );
  expect(order).toEqual([
    "submit-magic",
    "use-password",
    "register-pilot",
    "register-club",
  ]);
}

export async function expectPasswordRevealed(page: Page): Promise<void> {
  await expect(page.locator("#login-lead")).toHaveText("Inicia sesión con tu contraseña.");
  await expect(page.locator("#password-reveal")).toBeVisible();
  await expect(page.locator('input[name="email"][type="email"]')).toHaveCount(1);
  await expect(page.locator("#password")).toBeVisible();
  await expect(page.locator("#submit-password")).toBeVisible();
  await expect(page.locator("#submit-magic")).toBeHidden();
  const legacyDetails = page.locator("#legacy-details");
  await expect(legacyDetails).toBeVisible();
  await expect(legacyDetails).not.toHaveAttribute("open", /.*/);
}