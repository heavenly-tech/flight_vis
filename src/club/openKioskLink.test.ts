import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initI18n, setLocale } from "../i18n/index";
import { clubScreenPath, mountOpenKioskLink } from "./openKioskLink";

describe("openKioskLink", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("builds /c/{slug}/screen paths safely", () => {
    expect(clubScreenPath("demo")).toBe("/c/demo/screen");
    expect(clubScreenPath("  cas-tobalaba  ")).toBe("/c/cas-tobalaba/screen");
    expect(clubScreenPath("")).toBeNull();
    expect(clubScreenPath("   ")).toBeNull();
    expect(clubScreenPath(null)).toBeNull();
    expect(clubScreenPath(undefined)).toBeNull();
  });

  it("renders link immediately when clubSlug is passed without fetching /api/club/me", async () => {
    const slot = document.createElement("div");
    document.body.appendChild(slot);
    const fetchSpy = vi.fn();
    vi.stubGlobal("fetch", fetchSpy);

    const ctrl = await mountOpenKioskLink(slot, { clubSlug: "cas-demo" });
    expect(fetchSpy).not.toHaveBeenCalled();

    const link = slot.querySelector<HTMLAnchorElement>("a[data-open-kiosk-link]");
    expect(link).not.toBeNull();
    expect(link?.getAttribute("href")).toBe("/c/cas-demo/screen");
    expect(link?.target).toBe("_blank");
    expect(link?.rel).toBe("noopener");
    expect(link?.textContent).toBe("Abrir pantalla del club");
    expect(ctrl.getHref()).toBe("/c/cas-demo/screen");

    ctrl.updateSlug("nuevo-club");
    expect(link?.getAttribute("href")).toBe("/c/nuevo-club/screen");
    expect(ctrl.getHref()).toBe("/c/nuevo-club/screen");
  });

  it("fetches /api/club/me when clubSlug is omitted and updates label on locale change", async () => {
    const slot = document.createElement("div");
    document.body.appendChild(slot);
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        if (String(input) === "/api/club/me") {
          return new Response(JSON.stringify({ clubSlug: "demo" }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    await mountOpenKioskLink(slot);
    const link = slot.querySelector<HTMLAnchorElement>("a[data-open-kiosk-link]");
    expect(link?.getAttribute("href")).toBe("/c/demo/screen");
    expect(link?.textContent).toBe("Abrir pantalla del club");

    await setLocale("en", { persistUser: false });
    expect(link?.textContent).toBe("Open club screen");
  });

  it("stays empty gracefully when /api/club/me fails", async () => {
    const slot = document.createElement("div");
    document.body.appendChild(slot);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("{}", { status: 401 })),
    );

    const ctrl = await mountOpenKioskLink(slot);
    expect(ctrl.getHref()).toBeNull();
    expect(slot.querySelector("a")).toBeNull();
  });
});
