import { test, expect } from "@playwright/test";

test("signup routes serve signup page directly with 200 (no redirect)", async ({
  page,
  request,
}) => {
  for (const route of ["/signup", "/signup.html"]) {
    const raw = await request.get(route, { maxRedirects: 0 });
    expect(raw.status(), route).toBe(200);
    const res = await page.goto(route, { waitUntil: "domcontentloaded" });
    expect(res?.status(), route).toBe(200);
    await expect(page).toHaveURL(new RegExp(`${route.replace(".", "\\.")}$`));
    await expect(
      page.getByRole("heading", { name: /crea tu cuenta|create your account/i }),
    ).toBeVisible();
  }
});

test("signup capability endpoint", async ({ request }) => {
  const res = await request.get("/api/account/signup-available");
  expect(res.ok()).toBeTruthy();
  const body = await res.json();
  expect(body.signup).toBe(true);
});

test("public clubs endpoint exposes only name and slug", async ({ request }) => {
  const res = await request.get("/api/clubs/public");
  expect(res.ok()).toBeTruthy();
  const body = (await res.json()) as { clubs: Array<Record<string, unknown>> };
  expect(Array.isArray(body.clubs)).toBe(true);
  for (const club of body.clubs) {
    expect(Object.keys(club).sort()).toEqual(["name", "slug"]);
  }
});

test("signup optional club picker toggles select, lists only public clubs, and sends clubSlug only when checked", async ({
  page,
}) => {
  const listedClubs = [
    { slug: "andes-club", name: "Club Aéreo Los Andes" },
    { slug: "demo", name: "Club Demo" },
  ];
  const postedPayloads: Array<Record<string, unknown>> = [];

  await page.route("**/api/clubs/public", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ clubs: listedClubs }),
    });
  });

  await page.route("**/api/account/signup", async (route) => {
    const data = route.request().postDataJSON() as Record<string, unknown>;
    postedPayloads.push(data);
    await route.fulfill({
      status: 201,
      contentType: "application/json",
      body: JSON.stringify({ ok: true }),
    });
  });

  await page.goto("/signup", { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => (window as unknown as { __fvAccountSignupReady?: boolean }).__fvAccountSignupReady === true,
  );

  const toggle = page.locator("#is-club-member");
  const wrap = page.locator("#club-select-wrap");
  const select = page.locator("#club-slug");

  await expect(toggle).not.toBeChecked();
  await expect(wrap).toBeHidden();
  await expect(select).toBeDisabled();

  // Submit without toggling club membership -> must omit clubSlug.
  await page.locator('#form input[name="email"]').fill("pilot-no-club@example.test");
  await page.locator('#form button[type="submit"]').click();
  await expect.poll(() => postedPayloads.length).toBe(1);
  expect(postedPayloads[0]).toEqual({ email: "pilot-no-club@example.test" });
  expect("clubSlug" in postedPayloads[0]).toBe(false);

  // Toggle club membership -> select appears and contains only the listed clubs.
  await toggle.check();
  await expect(wrap).toBeVisible();
  await expect(select).toBeEnabled();
  await expect(select.locator("option")).toHaveCount(listedClubs.length + 1);
  const optionValues = await select
    .locator("option")
    .evaluateAll((opts) => opts.map((o) => (o as HTMLOptionElement).value));
  expect(optionValues).toEqual(["", "andes-club", "demo"]);

  // Pick a club and submit -> sends clubSlug in POST payload.
  await select.selectOption("andes-club");
  await page.locator('#form input[name="email"]').fill("pilot-with-club@example.test");
  await page.locator('#form button[type="submit"]').click();
  await expect.poll(() => postedPayloads.length).toBe(2);
  expect(postedPayloads[1]).toEqual({
    email: "pilot-with-club@example.test",
    clubSlug: "andes-club",
  });
});
