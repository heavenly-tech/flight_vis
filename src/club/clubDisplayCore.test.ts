import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CHECKIN_STEP_SEC } from "./checkinStep";
import { mountClubDisplayScreen } from "./clubDisplayCore";
import { KIOSK_SYNC_STORAGE_KEY } from "./kioskSync";
import { initI18n, t } from "../i18n/index";

class StubEventSource {
  static instances: StubEventSource[] = [];
  url: string;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(url: string) {
    this.url = url;
    StubEventSource.instances.push(this);
  }
  addEventListener(): void {}
  close(): void {
    this.closed = true;
  }
}

function buildKioskDom() {
  document.body.innerHTML = `
    <div class="kiosk-viewport" id="viewport">
      <header class="kiosk-header">
        <div class="kiosk-header-main">
          <h2 data-i18n="kiosk.liveAircraft">Aeronaves en vivo</h2>
          <p id="status-bar" data-i18n="kiosk.starting"></p>
        </div>
        <div id="toolbar-local">
          <button type="button" id="btn-local-qr" data-i18n="kiosk.qrHide" hidden></button>
        </div>
      </header>
      <section id="map-layer"><ul id="map"></ul></section>
      <div id="qr-wrap" hidden><img id="qr" alt="" /></div>
    </div>
  `;
  return {
    statusEl: document.querySelector<HTMLParagraphElement>("#status-bar")!,
    qr: document.querySelector<HTMLImageElement>("#qr")!,
    qrWrap: document.querySelector<HTMLDivElement>("#qr-wrap")!,
    mapLayer: document.querySelector<HTMLElement>("#map-layer")!,
    map: document.querySelector<HTMLUListElement>("#map")!,
    viewport: document.querySelector<HTMLDivElement>("#viewport")!,
    localToggleBtn: document.querySelector<HTMLButtonElement>("#btn-local-qr")!,
  };
}

describe("mountClubDisplayScreen session-authenticated loop", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    StubEventSource.instances = [];
    vi.stubGlobal("EventSource", StubEventSource);
    vi.stubGlobal("BroadcastChannel", undefined);
    initI18n({ stored: "es" });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("loads base, layout, QR, and map immediately via session cookies and refetches on sync signal", async () => {
    const dom = buildKioskDom();
    const redirectSpy = vi.fn();
    let mapPollCount = 0;
    let baseFetchCount = 0;
    let layoutFetchCount = 0;

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (url === "/api/display/base") {
        baseFetchCount += 1;
        return new Response(
          JSON.stringify({ icao: "SCTB", lat: -33.456, lon: -70.548, clubName: "Club Demo" }),
          { status: 200 },
        );
      }
      if (url === "/api/display/map") {
        mapPollCount += 1;
        const aircraft =
          mapPollCount === 1
            ? [{ tail: "CC-AAA", pilotName: "Piloto Uno" }]
            : [
                { tail: "CC-AAA", pilotName: "Piloto Uno" },
                {
                  tail: "CC-BBB",
                  pilotName: "Piloto Dos",
                  fix: { lat: -33.5, lon: -70.6 },
                },
              ];
        return new Response(JSON.stringify({ aircraft }), { status: 200 });
      }
      if (url === "/api/display/kiosk-layout") {
        layoutFetchCount += 1;
        return new Response(
          JSON.stringify({ x: 0.76, y: 0.76, size: 0.22, checkinQrVisible: true }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountClubDisplayScreen({ ...dom, onRedirectToLogin: redirectSpy });
    await vi.advanceTimersByTimeAsync(0);

    expect(redirectSpy).not.toHaveBeenCalled();
    expect(dom.localToggleBtn.hidden).toBe(false);
    expect(dom.localToggleBtn.textContent).toBe(t("kiosk.qrHide"));
    expect(dom.statusEl.textContent).toBe(
      t("kiosk.qrRotateHint", { seconds: String(CHECKIN_STEP_SEC) }),
    );
    expect(baseFetchCount).toBe(1);
    expect(layoutFetchCount).toBe(1);
    expect(mapPollCount).toBe(1);
    const kioskMap = dom.mapLayer.querySelector<HTMLElement>('[data-testid="kiosk-map"]');
    expect(kioskMap).not.toBeNull();
    expect(kioskMap?.getAttribute("data-view")).toBe("base");
    expect(kioskMap?.querySelectorAll("img[data-tile]").length).toBeGreaterThan(0);
    expect(kioskMap?.querySelector('[data-marker="base"]')?.textContent).toBe("SCTB");
    expect(dom.map.querySelectorAll("li")).toHaveLength(1);
    expect(dom.map.textContent).toContain("CC-AAA · Piloto Uno");
    expect(StubEventSource.instances).toHaveLength(1);

    // Cross-window refetch signal triggers immediate refetch without waiting for 5s poll.
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "refetch:1234:1-abc",
      }),
    );
    await vi.advanceTimersByTimeAsync(50);

    expect(baseFetchCount).toBe(2);
    expect(layoutFetchCount).toBe(2);
    expect(mapPollCount).toBe(2);
    expect(dom.map.querySelectorAll("li")).toHaveLength(2);
    expect(dom.map.textContent).toContain("CC-BBB · Piloto Dos");
    expect(kioskMap?.getAttribute("data-view")).toBe("flights");
    expect(kioskMap?.querySelector('[data-marker="aircraft"][data-tail="CC-BBB"]')).not.toBeNull();
  });

  it("redirects to login once on 401 and stops all polling loops with no error loop", async () => {
    const dom = buildKioskDom();
    const redirectSpy = vi.fn();
    let authorized = true;
    let mapCallsAfter401 = 0;

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (!authorized) {
        if (url === "/api/display/map") mapCallsAfter401 += 1;
        return new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 });
      }
      if (url === "/api/display/base") {
        return new Response(JSON.stringify({ icao: "SCTB", lat: -33.456, lon: -70.548 }), {
          status: 200,
        });
      }
      if (url === "/api/display/kiosk-layout") {
        return new Response(
          JSON.stringify({ x: 0.76, y: 0.76, size: 0.22, checkinQrVisible: true }),
          { status: 200 },
        );
      }
      if (url === "/api/display/map") {
        return new Response(JSON.stringify({ aircraft: [] }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountClubDisplayScreen({ ...dom, onRedirectToLogin: redirectSpy });
    await vi.advanceTimersByTimeAsync(0);
    expect(redirectSpy).not.toHaveBeenCalled();

    authorized = false;
    await vi.advanceTimersByTimeAsync(5000);
    expect(redirectSpy).toHaveBeenCalledTimes(1);
    expect(redirectSpy).toHaveBeenCalledWith("/login?next=%2Fclub-screen.html");
    expect(StubEventSource.instances[0]?.closed).toBe(true);

    // Further timer ticks or logout pings must NOT re-redirect or keep polling.
    const callsAtRedirect = mapCallsAfter401;
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "logout:999:2-xyz",
      }),
    );
    await vi.advanceTimersByTimeAsync(20_000);
    expect(redirectSpy).toHaveBeenCalledTimes(1);
    expect(mapCallsAfter401).toBe(callsAtRedirect);
  });

  it("redirects to login once on logout signal from admin window", async () => {
    const dom = buildKioskDom();
    const redirectSpy = vi.fn();

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (url === "/api/display/base") {
        return new Response(JSON.stringify({ icao: "SCTB", lat: -33.456, lon: -70.548 }), {
          status: 200,
        });
      }
      if (url === "/api/display/kiosk-layout") {
        return new Response(
          JSON.stringify({ x: 0.76, y: 0.76, size: 0.22, checkinQrVisible: true }),
          { status: 200 },
        );
      }
      if (url === "/api/display/map") {
        return new Response(JSON.stringify({ aircraft: [] }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountClubDisplayScreen({ ...dom, onRedirectToLogin: redirectSpy });
    await vi.advanceTimersByTimeAsync(0);

    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "logout:100:1-a",
      }),
    );
    window.dispatchEvent(
      new StorageEvent("storage", {
        key: KIOSK_SYNC_STORAGE_KEY,
        newValue: "logout:101:2-b",
      }),
    );

    expect(redirectSpy).toHaveBeenCalledTimes(1);
    expect(redirectSpy).toHaveBeenCalledWith("/login?next=%2Fclub-screen.html");
    expect(StubEventSource.instances[0]?.closed).toBe(true);
  });

  it("renders clubName, empty state, tail focus via button or marker, detail strip, auto-drop, and Ver todas reset", async () => {
    const dom = buildKioskDom();
    const redirectSpy = vi.fn();
    let pollStep = 0;

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (url === "/api/display/base") {
        return new Response(
          JSON.stringify({
            icao: "SCTB",
            lat: -33.456,
            lon: -70.548,
            clubName: "Club Aéreo de Santiago",
          }),
          { status: 200 },
        );
      }
      if (url === "/api/display/kiosk-layout") {
        return new Response(
          JSON.stringify({ x: 0.85, y: 0.76, size: 0.22, checkinQrVisible: true }),
          { status: 200 },
        );
      }
      if (url === "/api/display/map") {
        pollStep += 1;
        // 1 = initial empty poll, 2 = 2 aircraft, 3 = CC-AAA moved, 4 = CC-AAA gone
        if (pollStep === 1) {
          return new Response(JSON.stringify({ aircraft: [] }), { status: 200 });
        }
        if (pollStep === 2) {
          return new Response(
            JSON.stringify({
              aircraft: [
                {
                  tail: "CC-AAA",
                  pilotName: "Piloto Uno",
                  fix: { lat: -33.5, lon: -70.6, alt: 1249.6, speed: 40 },
                },
                {
                  tail: "CC-BBB",
                  fix: { lat: -33.42, lon: -70.5 },
                },
              ],
            }),
            { status: 200 },
          );
        }
        if (pollStep === 3) {
          return new Response(
            JSON.stringify({
              aircraft: [
                {
                  tail: "CC-AAA",
                  pilotName: "Piloto Uno",
                  fix: { lat: -33.51, lon: -70.61, alt: 1300, speed: 45 },
                },
                {
                  tail: "CC-BBB",
                  fix: { lat: -33.42, lon: -70.5 },
                },
              ],
            }),
            { status: 200 },
          );
        }
        return new Response(
          JSON.stringify({
            aircraft: [
              {
                tail: "CC-BBB",
                fix: { lat: -33.42, lon: -70.5 },
              },
            ],
          }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountClubDisplayScreen({ ...dom, onRedirectToLogin: redirectSpy });
    await vi.advanceTimersByTimeAsync(0);

    // Club name shown in header
    const clubNameEl = dom.viewport.querySelector<HTMLElement>("#club-name");
    expect(clubNameEl?.hidden).toBe(false);
    expect(clubNameEl?.textContent).toBe("Club Aéreo de Santiago");

    // Empty state shown when no aircraft
    const emptyEl = dom.viewport.querySelector<HTMLElement>("#map-empty");
    expect(emptyEl?.hidden).toBe(false);
    expect(emptyEl?.textContent).toBe(t("kiosk.noAircraft"));
    expect(dom.map.querySelectorAll("li")).toHaveLength(0);

    // Next 5s poll brings 2 aircraft
    await vi.advanceTimersByTimeAsync(5000);
    expect(emptyEl?.hidden).toBe(true);
    const buttons = dom.map.querySelectorAll<HTMLButtonElement>("li > button");
    expect(buttons).toHaveLength(2);
    expect(buttons[0].getAttribute("aria-pressed")).toBe("false");
    expect(buttons[1].getAttribute("aria-pressed")).toBe("false");

    const kioskMap = dom.mapLayer.querySelector<HTMLElement>('[data-testid="kiosk-map"]')!;
    const viewAllBtn = dom.viewport.querySelector<HTMLButtonElement>("#btn-view-all")!;
    const focusDetail = dom.viewport.querySelector<HTMLElement>("#focus-detail")!;
    expect(viewAllBtn.hidden).toBe(true);
    expect(focusDetail.hidden).toBe(true);

    // Tap first tail button -> focuses CC-AAA, shows detail strip with real payload data only
    buttons[0].click();
    expect(kioskMap.getAttribute("data-view")).toBe("focus");
    expect(kioskMap.getAttribute("data-focused-tail")).toBe("CC-AAA");
    expect(viewAllBtn.hidden).toBe(false);
    expect(viewAllBtn.textContent).toBe(t("kiosk.viewAll"));
    expect(focusDetail.hidden).toBe(false);
    expect(focusDetail.textContent).toContain("CC-AAA");
    expect(focusDetail.textContent).toContain("Piloto Uno");
    expect(focusDetail.textContent).toContain("1250 m");
    expect(focusDetail.textContent).toContain("144 km/h");
    expect(
      dom.map.querySelector('button[data-tail="CC-AAA"]')?.getAttribute("aria-pressed"),
    ).toBe("true");

    // Next 5s poll keeps following CC-AAA and updates detail strip
    await vi.advanceTimersByTimeAsync(5000);
    expect(kioskMap.getAttribute("data-view")).toBe("focus");
    expect(kioskMap.getAttribute("data-focused-tail")).toBe("CC-AAA");
    expect(focusDetail.textContent).toContain("1300 m");
    expect(focusDetail.textContent).toContain("162 km/h");

    // Click map marker for CC-BBB (pointer-events: auto) -> switches focus to CC-BBB, omits absent pilot/alt/speed
    const markerB = kioskMap.querySelector<HTMLButtonElement>(
      '[data-marker="aircraft"][data-tail="CC-BBB"]',
    )!;
    expect(markerB).not.toBeNull();
    expect(markerB.style.pointerEvents).toBe("auto");
    markerB.click();
    expect(kioskMap.getAttribute("data-focused-tail")).toBe("CC-BBB");
    expect(focusDetail.hidden).toBe(false);
    expect(focusDetail.textContent).toContain("CC-BBB");
    expect(focusDetail.querySelector('[data-detail="pilot"]')).toBeNull();
    expect(focusDetail.querySelector('[data-detail="alt"]')).toBeNull();
    expect(focusDetail.querySelector('[data-detail="speed"]')).toBeNull();

    // Click "Ver todas" resets to fit-all
    viewAllBtn.click();
    expect(kioskMap.getAttribute("data-view")).toBe("flights");
    expect(kioskMap.getAttribute("data-focused-tail")).toBeNull();
    expect(viewAllBtn.hidden).toBe(true);
    expect(focusDetail.hidden).toBe(true);

    // Focus CC-AAA again, then let poll 4 drop CC-AAA -> focus automatically clears
    dom.map.querySelector<HTMLButtonElement>('button[data-tail="CC-AAA"]')?.click();
    expect(kioskMap.getAttribute("data-focused-tail")).toBe("CC-AAA");
    await vi.advanceTimersByTimeAsync(5000);
    expect(kioskMap.getAttribute("data-focused-tail")).toBeNull();
    expect(kioskMap.getAttribute("data-view")).toBe("flights");
    expect(viewAllBtn.hidden).toBe(true);
    expect(focusDetail.hidden).toBe(true);
  });

  it("club-screen.html has no token activation form and keeps strict CSP markup", () => {
    const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
    const html = fs.readFileSync(path.join(root, "club-screen.html"), "utf8");
    const doc = new DOMParser().parseFromString(html, "text/html");
    expect(doc.querySelector("#activate")).toBeNull();
    expect(doc.querySelector("#activate-token")).toBeNull();
    expect(doc.querySelector("#btn-activate")).toBeNull();
    expect(doc.querySelector("#map-layer")).not.toBeNull();
    expect(doc.querySelector("#qr-wrap")).not.toBeNull();
  });
});
