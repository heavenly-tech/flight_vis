import { expect, test } from "@playwright/test";

test("root with one public live flight opens that flight", async ({ page }) => {
  await page.addInitScript(() => {
    const orig = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/api/public/live-summary")) {
        return new Response(
          JSON.stringify({ flights: [{ flightId: "live-one", tail: "N1" }] }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      if (url.includes("/api/auth/me")) {
        return new Response(JSON.stringify({ authenticated: false, configured: true }), {
          status: 200,
        });
      }
      return orig(input, init);
    };
  });

  await Promise.all([
    page.waitForURL(/club-flight\.html\?id=live-one/, { timeout: 15_000 }),
    page.goto("/"),
  ]);
});

test("/map skips live-summary redirect", async ({ page }) => {
  let summaryHits = 0;
  await page.addInitScript(() => {
    const orig = globalThis.fetch.bind(globalThis);
    globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.includes("/api/public/live-summary")) {
        (globalThis as unknown as { __summaryHits: number }).__summaryHits =
          ((globalThis as unknown as { __summaryHits?: number }).__summaryHits ?? 0) + 1;
        return new Response(JSON.stringify({ flights: [{ flightId: "x", tail: "N1" }] }), {
          status: 200,
        });
      }
      return orig(input, init);
    };
  });

  await page.goto("/map");
  await expect(page).toHaveURL(/\/map\/?$/, { timeout: 15_000 });
  summaryHits = await page.evaluate(
    () => (globalThis as unknown as { __summaryHits?: number }).__summaryHits ?? 0,
  );
  expect(summaryHits).toBe(0);
});