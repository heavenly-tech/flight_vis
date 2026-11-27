import { expect, test } from "@playwright/test";
import { HTML_GLOBE_CSP } from "./csp-constants";
import { hooksUrl, installCspObservers, stubArcgisTiles } from "./globe-csp-helpers";

type BridgeEvent =
  | { type: "ready"; protocol: 1; version: string }
  | { type: "followChanged"; on: false; reason: "gesture" }
  | { type: "error"; code: string }
  | { type: "perf"; quality: number };

const EMBED_PATH = "/embed/3d?ui=mobile&v=1";

test.describe("mobile 3D embed (/embed/3d)", () => {
  test.describe.configure({ mode: "serial" });
  test.setTimeout(120_000);
  test.use({
    viewport: { width: 390, height: 844 },
    hasTouch: true,
    isMobile: true,
  });

  test("headers on /embed/3d and /embed-3d.html include Globe CSP, no-referrer, and no-cache", async ({
    request,
  }) => {
    for (const path of [EMBED_PATH, "/embed-3d.html"]) {
      const res = await request.get(path);
      expect(res.status(), path).toBe(200);
      expect(res.headers()["content-security-policy"], `${path} CSP`).toBe(HTML_GLOBE_CSP);
      expect(res.headers()["content-security-policy"]).toContain("frame-ancestors 'none'");
      expect(res.headers()["referrer-policy"], `${path} Referrer-Policy`).toBe("no-referrer");
      expect(res.headers()["cache-control"], `${path} Cache-Control`).toBe("no-cache");
      const html = await res.text();
      expect(html).toContain('<meta name="robots" content="noindex"');
      expect(html).toContain("user-scalable=no");
      expect(html).not.toMatch(/<script(?![^>]*\ssrc=)/);
    }
  });

  test("bridge lifecycle, multi-aircraft delta, follow/recenter/gesture, strict network & zero chrome", async ({
    page,
  }) => {
    const apiRequests: string[] = [];
    const cookieRequests: string[] = [];
    const unexpectedHosts: string[] = [];
    const allowedThirdParty = new Set([
      "services.arcgisonline.com",
      "elevation3d.arcgis.com",
    ]);

    page.on("request", (req) => {
      const url = new URL(req.url());
      if (url.pathname.startsWith("/api/") || url.pathname === "/api") {
        apiRequests.push(req.url());
      }
      const headers = req.headers();
      if (headers.cookie) {
        cookieRequests.push(`${req.url()} (cookie present)`);
      }
      if (
        url.hostname !== "127.0.0.1" &&
        url.hostname !== "localhost" &&
        !allowedThirdParty.has(url.hostname)
      ) {
        unexpectedHosts.push(req.url());
      }
    });

    await stubArcgisTiles(page);
    const { consoleErrors, pageErrors } = installCspObservers(page);
    const allConsoleErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() !== "error") return;
      const text = msg.text();
      // Exclude known ArcGIS imagery/terrain tile noise used by existing globe specs.
      if (/arcgisonline|elevation3d|ArcGISTiledElevationTerrainProvider/i.test(text)) return;
      allConsoleErrors.push(text);
    });

    // Record all outbound events from both CustomEvent("fvbridge") and FvBridge.postMessage.
    await page.addInitScript(() => {
      const w = window as unknown as {
        __events: unknown[];
        __channelMessages: string[];
        FvBridge: { postMessage: (msg: string) => void };
      };
      w.__events = [];
      w.__channelMessages = [];
      w.FvBridge = {
        postMessage(msg: string) {
          w.__channelMessages.push(msg);
        },
      };
      window.addEventListener("fvbridge", (ev) => {
        w.__events.push((ev as CustomEvent).detail);
      });
    });

    await page.goto(hooksUrl(EMBED_PATH));

    // Wait for bridge ready event.
    await page.waitForFunction(
      () => {
        const events = (window as unknown as { __events?: BridgeEvent[] }).__events ?? [];
        return events.some((e) => e.type === "ready" || e.type === "error");
      },
      { timeout: 60_000 },
    );

    const bootEvents = await page.evaluate(
      () => (window as unknown as { __events: BridgeEvent[] }).__events,
    );
    const readyEvent = bootEvents.find((e) => e.type === "ready");
    expect(readyEvent, `expected ready event, got ${JSON.stringify(bootEvents)}`).toBeTruthy();
    expect(readyEvent).toMatchObject({ type: "ready", protocol: 1 });
    expect(typeof (readyEvent as { version: string }).version).toBe("string");

    // Verify FvBridge.postMessage received the exact JSON string too.
    const channelRaw = await page.evaluate(
      () => (window as unknown as { __channelMessages: string[] }).__channelMessages,
    );
    expect(channelRaw.some((s) => JSON.parse(s).type === "ready")).toBe(true);

    // Assert zero desktop chrome elements exist.
    for (const sel of [
      "#ui",
      ".topbar",
      ".transport",
      "#hud",
      "#profile",
      "#timeline-scrub",
      "#tags-panel",
      "#library-panel",
      'a[href="/login"]',
    ]) {
      await expect(page.locator(sel), `desktop chrome ${sel} must be absent`).toHaveCount(0);
    }

    // Compact credits container visible and container has touch-action: none.
    await expect(page.locator("#credits")).toBeVisible();
    const touchAction = await page.evaluate(
      () => getComputedStyle(document.getElementById("cesiumContainer")!).touchAction,
    );
    expect(touchAction).toBe("none");

    // Wrong protocol -> error event; valid init -> accepted.
    const badOk = await page.evaluate(() =>
      window.fvBridge!.receive({ type: "init", protocol: 99, lang: "es" }),
    );
    expect(badOk).toBe(false);

    const initOk = await page.evaluate(() =>
      window.fvBridge!.receive(
        JSON.stringify({
          type: "init",
          protocol: 1,
          lang: "es",
          ownFlightId: "f-own",
          units: "ft",
        }),
      ),
    );
    expect(initOk).toBe(true);
    expect(await page.evaluate(() => document.documentElement.lang)).toBe("es-CL");

    // Full snapshot with 3 aircraft (own, club, public/pilot-entered).
    const fullOk = await page.evaluate(() =>
      window.fvBridge!.receive({
        type: "flights",
        mode: "full",
        flights: [
          {
            id: "f-own",
            tail: "CC-OWN",
            tailSource: "club",
            layer: "own",
            points: [
              [1_700_000_000_000, -33.45, -70.66, 1100],
              [1_700_000_005_000, -33.445, -70.655, 1200],
            ],
          },
          {
            id: "f-club",
            tail: "CC-CLB",
            tailSource: "club",
            layer: "club",
            points: [
              [1_700_000_000_000, -33.42, -70.62, 1300],
              [1_700_000_005_000, -33.418, -70.618, 1320],
            ],
          },
          {
            id: "f-pub",
            tail: "CC-PIL",
            tailSource: "pilot",
            layer: "public",
            points: [
              [1_700_000_000_000, -33.48, -70.70, 950],
              [1_700_000_005_000, -33.479, -70.698, null],
            ],
          },
        ],
      }),
    );
    expect(fullOk).toBe(true);

    const afterFull = await page.evaluate(() => ({
      flightCount: window.__fvEmbed!.flightCount,
      ownPts: window.__fvEmbed!.pointCount("f-own"),
      clubPts: window.__fvEmbed!.pointCount("f-club"),
      pubPts: window.__fvEmbed!.pointCount("f-pub"),
    }));
    expect(afterFull).toEqual({ flightCount: 3, ownPts: 2, clubPts: 2, pubPts: 2 });

    // Delta appends points to f-own (including one out-of-range alt point that must be dropped).
    const deltaOk = await page.evaluate(() =>
      window.fvBridge!.receive({
        type: "flights",
        mode: "delta",
        flights: [
          {
            id: "f-own",
            tail: "CC-OWN",
            tailSource: "club",
            layer: "own",
            points: [
              [1_700_000_010_000, -33.44, -70.65, 1250],
              [1_700_000_012_000, -33.438, -70.648, 25_000], // dropped (> 20000m)
              [1_700_000_015_000, -33.435, -70.645, 1300],
            ],
          },
        ],
      }),
    );
    expect(deltaOk).toBe(true);
    expect(await page.evaluate(() => window.__fvEmbed!.pointCount("f-own"))).toBe(4);

    // Follow / recenter / overview state toggling.
    await page.evaluate(() => {
      window.fvBridge!.receive({ type: "follow", on: true, flightId: "f-own" });
    });
    expect(await page.evaluate(() => window.__fvEmbed!.following)).toBe(true);

    await page.evaluate(() => {
      window.fvBridge!.receive({ type: "recenter" });
    });
    expect(await page.evaluate(() => window.__fvEmbed!.following)).toBe(true);

    await page.evaluate(() => {
      window.fvBridge!.receive({ type: "overview" });
    });
    expect(await page.evaluate(() => window.__fvEmbed!.following)).toBe(false);

    // Re-enable follow, then simulate a drag gesture -> breaks follow & emits followChanged.
    await page.evaluate(() => {
      window.fvBridge!.receive({ type: "follow", on: true });
    });
    expect(await page.evaluate(() => window.__fvEmbed!.following)).toBe(true);

    const box = (await page.locator("#cesiumContainer").boundingBox())!;
    expect(box).not.toBeNull();
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.mouse.move(cx + 60, cy + 40, { steps: 6 });
    await page.mouse.up();

    await page.waitForFunction(() => window.__fvEmbed?.following === false, { timeout: 10_000 });

    const allEvents = await page.evaluate(
      () => (window as unknown as { __events: BridgeEvent[] }).__events,
    );
    expect(allEvents).toContainEqual({ type: "error", code: "unsupported_protocol" });
    expect(allEvents).toContainEqual({
      type: "followChanged",
      on: false,
      reason: "gesture",
    });

    // Pause / resume works cleanly.
    await page.evaluate(() => {
      window.fvBridge!.receive({ type: "pause" });
      window.fvBridge!.receive({ type: "resume" });
    });

    // Wait for at least one paint (in WebGL or stub).
    await page.waitForFunction(() => window.__fvEmbed?.painted === true, { timeout: 30_000 });

    // Strict CSP, console/page errors, and zero API/cookie/foreign requests.
    const violations = await page.evaluate(() => {
      if (!window.__cspViolations) {
        throw new Error("missing window.__cspViolations (e2e-csp-hooks.js)");
      }
      return window.__cspViolations;
    });
    expect(violations).toEqual([]);
    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
    expect(allConsoleErrors).toEqual([]);
    expect(apiRequests).toEqual([]);
    expect(cookieRequests).toEqual([]);
    expect(unexpectedHosts).toEqual([]);
  });

  test("does not expose window.__fvEmbed when e2eGlobeHooks query flag is absent", async ({
    page,
  }) => {
    await stubArcgisTiles(page);
    await page.goto(EMBED_PATH);
    await page.waitForFunction(() => typeof window.fvBridge?.receive === "function", {
      timeout: 30_000,
    });
    const hasHook = await page.evaluate(() => typeof window.__fvEmbed !== "undefined");
    expect(hasHook).toBe(false);
  });
});
