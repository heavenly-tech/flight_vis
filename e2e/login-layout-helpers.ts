import { expect, type Page } from "@playwright/test";

export async function expectSingleFormLayout(page: Page): Promise<void> {
  await expect(page.locator('input[name="email"][type="email"]')).toHaveCount(1);
  await expect(page.locator("#submit-magic")).toBeVisible();
  await expect(page.locator("#use-password")).toBeVisible();
  await expect(page.locator("#register-pilot")).toBeVisible();
  await expect(page.locator("#register-club")).toBeVisible();
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
  await expect(page.locator("#password-reveal")).toBeVisible();
  await expect(page.locator('input[name="email"][type="email"]')).toHaveCount(1);
  await expect(page.locator("#password")).toBeVisible();
  await expect(page.locator("#submit-password")).toBeVisible();
  await expect(page.locator("#submit-magic")).toBeHidden();
}