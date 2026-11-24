import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CHECKIN_STEP_SEC } from "./checkinStep";
import { mountClubDisplayScreen } from "./clubDisplayCore";
import { initI18n, t } from "../i18n/index";

class StubEventSource {
  static instances: StubEventSource[] = [];
  url: string;
  onerror: (() => void) | null = null;
  constructor(url: string) {
    this.url = url;
    StubEventSource.instances.push(this);
  }
  addEventListener(): void {}
  close(): void {}
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
      <div id="activate" hidden>
        <form id="activate-inner">
          <label for="activate-token" data-i18n="kiosk.pasteToken"></label>
          <input id="activate-token" type="password" />
          <button type="submit" id="btn-activate" data-i18n="kiosk.activate"></button>
        </form>
      </div>
    </div>
  `;
  return {
    statusEl: document.querySelector<HTMLParagraphElement>("#status-bar")!,
    qr: document.querySelector<HTMLImageElement>("#qr")!,
    qrWrap: document.querySelector<HTMLDivElement>("#qr-wrap")!,
    mapLayer: document.querySelector<HTMLElement>("#map-layer")!,
    map: document.querySelector<HTMLUListElement>("#map")!,
    viewport: document.querySelector<HTMLDivElement>("#viewport")!,
    activatePanel: document.querySelector<HTMLDivElement>("#activate")!,
    activateForm: document.querySelector<HTMLFormElement>("#activate-inner")!,
    activateToken: document.querySelector<HTMLInputElement>("#activate-token")!,
    activateBtn: document.querySelector<HTMLButtonElement>("#btn-activate")!,
    localToggleBtn: document.querySelector<HTMLButtonElement>("#btn-local-qr")!,
  };
}

describe("mountClubDisplayScreen activation loop", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    StubEventSource.instances = [];
    vi.stubGlobal("EventSource", StubEventSource);
    initI18n({ stored: "es" });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("starts polling, shows QR toggle, and updates status immediately on form submit with no reload", async () => {
    const dom = buildKioskDom();
    let activated = false;
    let mapPollCount = 0;
    let baseFetchCount = 0;

    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === "/api/club/i18n/context") {
        return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
      }
      if (url === "/api/display/activate") {
        activated = true;
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (url === "/api/display/base") {
        if (!activated) {
          return new Response(JSON.stringify({ error: "display session required" }), {
            status: 401,
          });
        }
        baseFetchCount += 1;
        return new Response(
          JSON.stringify({ icao: "SCTB", lat: -33.456, lon: -70.548 }),
          { status: 200 },
        );
      }
      if (url === "/api/display/map") {
        if (!activated) {
          return new Response(JSON.stringify({ error: "display session required" }), {
            status: 401,
          });
        }
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
        return new Response(
          JSON.stringify({ x: 0.76, y: 0.76, size: 0.22, checkinQrVisible: true }),
          { status: 200 },
        );
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountClubDisplayScreen(dom);
    await vi.advanceTimersByTimeAsync(0);

    expect(dom.activatePanel.hidden).toBe(false);
    expect(dom.localToggleBtn.hidden).toBe(true);
    expect(dom.statusEl.textContent).toBe(t("kiosk.awaitingActivation"));
    expect(dom.statusEl.textContent).not.toBe(t("kiosk.pasteToken"));

    dom.activateToken.value = "secret-display-token";
    dom.activateForm.dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.advanceTimersByTimeAsync(0);

    expect(dom.activatePanel.hidden).toBe(true);
    expect(dom.localToggleBtn.hidden).toBe(false);
    expect(dom.localToggleBtn.textContent).toBe(t("kiosk.qrHide"));
    expect(dom.statusEl.textContent).toBe(
      t("kiosk.qrRotateHint", { seconds: String(CHECKIN_STEP_SEC) }),
    );
    expect(baseFetchCount).toBe(1);
    const kioskMap = dom.mapLayer.querySelector<HTMLElement>('[data-testid="kiosk-map"]');
    expect(kioskMap).not.toBeNull();
    expect(kioskMap?.getAttribute("data-view")).toBe("base");
    expect(kioskMap?.querySelectorAll("img[data-tile]").length).toBeGreaterThan(0);
    expect(kioskMap?.querySelector('[data-marker="base"]')?.textContent).toBe("SCTB");
    expect(dom.map.querySelectorAll("li")).toHaveLength(1);
    expect(dom.map.textContent).toContain("CC-AAA · Piloto Uno");
    expect(StubEventSource.instances).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(5000);
    expect(mapPollCount).toBeGreaterThanOrEqual(2);
    expect(dom.map.querySelectorAll("li")).toHaveLength(2);
    expect(dom.map.textContent).toContain("CC-BBB · Piloto Dos");
    expect(kioskMap?.getAttribute("data-view")).toBe("flights");
    expect(kioskMap?.querySelector('[data-marker="aircraft"][data-tail="CC-BBB"]')).not.toBeNull();
  });
});
