import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mountClubNearbyPublicSetting, mountClubTrailSetting } from "./adminMap";
import {
  ALLOWED_TRAIL_MINUTES,
  buildTrailSegments,
  computeTrailLengthPx,
  isValidTrailMinutes,
  parseTrailChipAttribute,
  parseTrailMinutes,
  parseTrailsPayload,
  prepareTrailPoints,
  TRAIL_ALL,
  TRAIL_OFF,
  trailMinutesWindowMs,
  trailOptionI18n,
  type TrailPoint,
} from "./liveTrail";
import { extractPublicAircraft, renderClubTileMap } from "./tileMapRenderer";
import { initI18n } from "../i18n/index";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("liveTrail pure helpers", () => {
  it("validates allowed trail values and rejects invalid ones", () => {
    for (const v of [0, 2, 5, 10, 15, 30, -1]) {
      expect(isValidTrailMinutes(v)).toBe(true);
      expect(parseTrailMinutes(v)).toBe(v);
      expect(parseTrailChipAttribute(String(v))).toBe(v);
    }
    for (const bad of [-2, 1, 3, 7, 45, 60, 5.5, "5", null, undefined, Number.NaN]) {
      expect(isValidTrailMinutes(bad)).toBe(false);
      expect(parseTrailMinutes(bad, 5)).toBe(5);
    }
    expect(parseTrailChipAttribute("7")).toBeNull();
    expect(parseTrailChipAttribute("")).toBeNull();
    expect(parseTrailChipAttribute(null)).toBeNull();
  });

  it("maps trail chip values to i18n keys and windows", () => {
    expect(trailOptionI18n(TRAIL_OFF)).toEqual({ key: "trail.off" });
    expect(trailOptionI18n(5)).toEqual({ key: "trail.minutes", params: { minutes: "5" } });
    expect(trailOptionI18n(TRAIL_ALL)).toEqual({ key: "trail.all" });

    expect(trailMinutesWindowMs(TRAIL_OFF)).toBeNull();
    expect(trailMinutesWindowMs(2)).toBe(120_000);
    expect(trailMinutesWindowMs(30)).toBe(1_800_000);
    expect(trailMinutesWindowMs(TRAIL_ALL)).toBe(120 * 60_000);
  });

  it("parses trails payload filtering malformed rows and invalid coordinates", () => {
    const parsed = parseTrailsPayload({
      trails: [
        {
          flightId: "f-1",
          tail: "CC-AAA",
          trailMinutes: 10,
          points: [
            [1000, -33.45, -70.55],
            [2000, 999, -70.55],
            ["bad", -33.46, -70.56],
            [3000, -33.46, -70.56],
          ],
        },
        { flightId: "f-bad", tail: "" },
        null,
      ],
    });
    expect(parsed).toEqual([
      {
        flightId: "f-1",
        tail: "CC-AAA",
        trailMinutes: 10,
        points: [
          [1000, -33.45, -70.55],
          [3000, -33.46, -70.56],
        ],
      },
    ]);
  });

  it("prepareTrailPoints trims to window, handles Off/All, and attaches live marker fix", () => {
    const now = 1_700_000_000_000;
    const pts: TrailPoint[] = [
      [now - 130 * 60_000, -33.6, -70.7], // older than 120 min
      [now - 20 * 60_000, -33.55, -70.65],
      [now - 4 * 60_000, -33.5, -70.6],
      [now - 1 * 60_000, -33.47, -70.57],
    ];

    expect(prepareTrailPoints(pts, TRAIL_OFF, { t: now, lat: -33.45, lon: -70.55 })).toEqual([]);

    const fiveMin = prepareTrailPoints(pts, 5, { t: now, lat: -33.45, lon: -70.55 });
    expect(fiveMin.map((p) => p[0])).toEqual([now - 4 * 60_000, now - 1 * 60_000, now]);
    expect(fiveMin[fiveMin.length - 1]).toEqual([now, -33.45, -70.55]);

    const twoMin = prepareTrailPoints(pts, 2, { t: now, lat: -33.45, lon: -70.55 });
    expect(twoMin.map((p) => p[0])).toEqual([now - 1 * 60_000, now]);

    const allCapped = prepareTrailPoints(pts, TRAIL_ALL, { t: now, lat: -33.45, lon: -70.55 });
    expect(allCapped.map((p) => p[0])).toEqual([
      now - 20 * 60_000,
      now - 4 * 60_000,
      now - 1 * 60_000,
      now,
    ]);
  });

  it("buildTrailSegments creates ascending alpha ramp oldest -> newest and computes length", () => {
    expect(buildTrailSegments([])).toEqual([]);
    expect(buildTrailSegments([{ x: 10, y: 10 }])).toEqual([]);

    const single = buildTrailSegments(
      [
        { x: 0, y: 0 },
        { x: 3, y: 4 },
      ],
      0.2,
      0.9,
    );
    expect(single).toHaveLength(1);
    expect(single[0].alpha).toBeCloseTo(0.9, 5);
    expect(
      computeTrailLengthPx([
        { x: 0, y: 0 },
        { x: 3, y: 4 },
      ]),
    ).toBeCloseTo(5, 5);

    const multi = buildTrailSegments(
      [
        { x: 0, y: 0 },
        { x: 10, y: 0 },
        { x: 20, y: 0 },
        { x: 30, y: 0 },
      ],
      0.1,
      1.0,
    );
    expect(multi).toHaveLength(3);
    expect(multi[0].alpha).toBeCloseTo(0.1, 5);
    expect(multi[1].alpha).toBeCloseTo(0.55, 5);
    expect(multi[2].alpha).toBeCloseTo(1.0, 5);
    expect(multi[0].alpha).toBeLessThan(multi[1].alpha);
    expect(multi[1].alpha).toBeLessThan(multi[2].alpha);
  });

  it("renderClubTileMap renders trail metadata elements for active trails and omits Off trails", () => {
    const now = 1_700_000_000_000;
    const container = document.createElement("div");
    renderClubTileMap(container, {
      base: { icao: "SCTB", lat: -33.456, lon: -70.548 },
      aircraft: [
        { tail: "CC-AAA", flightId: "f1", lat: -33.45, lon: -70.54, fixT: now, trailMinutes: 5 },
        { tail: "CC-OFF", flightId: "f2", lat: -33.47, lon: -70.56, fixT: now, trailMinutes: 0 },
      ],
      trails: [
        {
          flightId: "f1",
          tail: "CC-AAA",
          trailMinutes: 5,
          points: [
            [now - 180_000, -33.47, -70.56],
            [now - 60_000, -33.46, -70.55],
            [now, -33.45, -70.54],
          ],
        },
        {
          flightId: "f2",
          tail: "CC-OFF",
          trailMinutes: 0,
          points: [],
        },
      ],
      viewportPx: { w: 800, h: 450 },
    });

    const trails = container.querySelectorAll('[data-testid="live-trail"]');
    expect(trails).toHaveLength(1);
    expect(trails[0].getAttribute("data-trail-tail")).toBe("CC-AAA");
    expect(trails[0].getAttribute("data-trail-minutes")).toBe("5");
    expect(trails[0].getAttribute("data-trail-points")).toBe("3");
    expect(Number(trails[0].getAttribute("data-trail-length-px"))).toBeGreaterThan(0);
  });
});

describe("club trail chip setting control", () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
    document.body.innerHTML = "";
  });

  function mountClubSettingsHtml(): void {
    initI18n({ stored: "es" });
    const html = fs.readFileSync(path.join(root, "club-admin-map.html"), "utf8");
    const doc = new DOMParser().parseFromString(html, "text/html");
    const details = doc.querySelector("#club-settings");
    expect(details).not.toBeNull();
    document.body.appendChild(document.importNode(details!, true));
  }

  it("renders es-CL chips, handles admin PATCH, calls onSaved, and relocalizes to en", async () => {
    mountClubSettingsHtml();
    const onSaved = vi.fn();
    const ctrl = mountClubTrailSetting(onSaved);

    const chips = () =>
      Array.from(document.querySelectorAll<HTMLButtonElement>("#club-trail-chips button"));

    // Disabled until configured as admin
    expect(chips()).toHaveLength(ALLOWED_TRAIL_MINUTES.length);
    expect(chips().every((b) => b.disabled)).toBe(true);

    ctrl.configure({ isAdmin: true, trailMinutes: 5 });
    expect(chips().map((b) => b.textContent)).toEqual([
      "No",
      "2 min",
      "5 min",
      "10 min",
      "15 min",
      "30 min",
      "Todo",
    ]);
    expect(
      document
        .querySelector('#club-trail-chips button[data-trail-value="5"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");

    const fetchMock = vi.fn(async () => ({
      ok: true,
      json: async () => ({ trailMinutes: 15 }),
    }));
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    const btn15 = document.querySelector<HTMLButtonElement>(
      '#club-trail-chips button[data-trail-value="15"]',
    )!;
    btn15.click();

    await vi.waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(fetchMock).toHaveBeenCalledWith("/api/admin/club/trail", {
      method: "PATCH",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ trailMinutes: 15 }),
    });
    expect(
      document
        .querySelector('#club-trail-chips button[data-trail-value="15"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    expect(document.querySelector("#club-trail-msg")?.textContent).toBe(
      "Estela predeterminada guardada.",
    );

    initI18n({ stored: "en" });
    ctrl.relocalize();
    expect(chips().map((b) => b.textContent)).toEqual([
      "Off",
      "2 min",
      "5 min",
      "10 min",
      "15 min",
      "30 min",
      "All",
    ]);
  });

  it("reverts pressed chip and displays error alert on failed PATCH", async () => {
    mountClubSettingsHtml();
    const onSaved = vi.fn();
    const ctrl = mountClubTrailSetting(onSaved);
    ctrl.configure({ isAdmin: true, trailMinutes: 5 });

    globalThis.fetch = vi.fn(async () => ({
      ok: false,
      status: 403,
      json: async () => ({ error: "admin only" }),
    })) as unknown as typeof fetch;

    const btn30 = document.querySelector<HTMLButtonElement>(
      '#club-trail-chips button[data-trail-value="30"]',
    )!;
    btn30.click();

    await vi.waitFor(() =>
      expect(document.querySelector("#club-trail-msg")?.getAttribute("role")).toBe("alert"),
    );
    expect(onSaved).not.toHaveBeenCalled();
    expect(
      document
        .querySelector('#club-trail-chips button[data-trail-value="5"]')
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
  });
});

describe("nearby public flights rendering and settings control", () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
    document.body.innerHTML = "";
  });

  function mountClubSettingsHtml(locale: "es" | "en" = "es"): void {
    initI18n({ stored: locale });
    const html = fs.readFileSync(path.join(root, "club-admin-map.html"), "utf8");
    const doc = new DOMParser().parseFromString(html, "text/html");
    const details = doc.querySelector("#club-settings");
    expect(details).not.toBeNull();
    document.body.appendChild(document.importNode(details!, true));
  }

  it("parses nonClub flag and renders muted non-club marker, badge, and trail metadata", () => {
    initI18n({ stored: "es" });
    const now = 1_700_000_000_000;
    const aircraft = extractPublicAircraft([
      {
        tail: "N101",
        flightId: "f-own",
        trailMinutes: 5,
        fix: { t: now, lat: -33.45, lon: -70.54 },
      },
      {
        tail: "CC-EXT",
        flightId: "f-ext",
        trailMinutes: 10,
        nonClub: true,
        fix: { t: now, lat: -33.42, lon: -70.51 },
      },
    ]);
    expect(aircraft[0].nonClub).toBeUndefined();
    expect(aircraft[1].nonClub).toBe(true);

    const trails = parseTrailsPayload({
      trails: [
        {
          flightId: "f-ext",
          tail: "CC-EXT",
          trailMinutes: 10,
          nonClub: true,
          points: [
            [now - 120_000, -33.43, -70.52],
            [now, -33.42, -70.51],
          ],
        },
      ],
    });
    expect(trails[0].nonClub).toBe(true);

    const container = document.createElement("div");
    renderClubTileMap(container, {
      base: { icao: "SCTB", lat: -33.456, lon: -70.548 },
      aircraft,
      trails,
      viewportPx: { w: 800, h: 450 },
    });

    const extMarker = container.querySelector('[data-marker="aircraft"][data-tail="CC-EXT"]');
    expect(extMarker?.getAttribute("data-non-club")).toBe("true");
    expect(extMarker?.classList.contains("is-non-club")).toBe(true);
    expect(extMarker?.querySelector('[data-non-club-label="true"]')?.textContent).toBe(
      "Fuera del club",
    );

    const ownMarker = container.querySelector('[data-marker="aircraft"][data-tail="N101"]');
    expect(ownMarker?.getAttribute("data-non-club")).toBeNull();

    const extTrail = container.querySelector('[data-testid="live-trail"][data-trail-tail="CC-EXT"]');
    expect(extTrail?.getAttribute("data-non-club")).toBe("true");
  });

  it("toggles nearby public setting, changes radius, calls onSaved, and reverts on failure", async () => {
    mountClubSettingsHtml("es");
    const onSaved = vi.fn();
    const ctrl = mountClubNearbyPublicSetting(onSaved);

    const toggle = document.querySelector<HTMLInputElement>("#club-nearby-public-toggle")!;
    const radius = document.querySelector<HTMLSelectElement>("#club-nearby-public-radius")!;
    const msg = document.querySelector<HTMLParagraphElement>("#club-nearby-public-msg")!;

    expect(toggle.disabled).toBe(true);
    expect(radius.disabled).toBe(true);

    ctrl.configure({ isAdmin: true, showNearbyPublic: true, nearbyPublicRadiusKm: 50 });
    expect(toggle.disabled).toBe(false);
    expect(toggle.checked).toBe(true);
    expect(radius.disabled).toBe(false);
    expect(radius.value).toBe("50");

    const fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body ?? "{}")) as {
        showNearbyPublic: boolean;
        nearbyPublicRadiusKm: number;
      };
      return {
        ok: true,
        json: async () => body,
      };
    });
    globalThis.fetch = fetchMock as unknown as typeof fetch;

    toggle.checked = false;
    toggle.dispatchEvent(new Event("change"));

    await vi.waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(fetchMock).toHaveBeenCalledWith("/api/admin/club/nearby-public", {
      method: "PATCH",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ showNearbyPublic: false, nearbyPublicRadiusKm: 50 }),
    });
    expect(radius.disabled).toBe(true);
    expect(msg.textContent).toBe("Configuración de vuelos cercanos guardada.");

    // Failed PATCH reverts toggle
    globalThis.fetch = vi.fn(async () => ({
      ok: false,
      status: 403,
      json: async () => ({ error: "admin only" }),
    })) as unknown as typeof fetch;

    toggle.checked = true;
    toggle.dispatchEvent(new Event("change"));
    await vi.waitFor(() => expect(msg.getAttribute("role")).toBe("alert"));
    expect(toggle.checked).toBe(false);
    expect(onSaved).toHaveBeenCalledTimes(1);
  });
});
