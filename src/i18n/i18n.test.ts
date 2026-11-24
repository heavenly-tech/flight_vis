import { describe, expect, it, vi } from "vitest";
import { flattenCatalog, bundledLocaleIds, CANONICAL_LOCALE } from "./catalog";
import {
  initI18n,
  t,
  resolveLocale,
  apiErrorMessage,
  setLocale,
  getLocale,
  applyI18nDocument,
} from "./index";

describe("i18n catalogs", () => {
  it("bundled locales share canonical keys", () => {
    const canon = Object.keys(flattenCatalog(CANONICAL_LOCALE)).sort();
    for (const id of bundledLocaleIds()) {
      const keys = Object.keys(flattenCatalog(id)).sort();
      expect(keys).toEqual(canon);
    }
  });

  it("defaults to Spanish", () => {
    vi.stubGlobal("navigator", { language: "es-CL" });
    initI18n();
    expect(t("entrar.title")).toMatch(/iniciar sesión/i);
    vi.unstubAllGlobals();
  });

  it("resolves stored locale over pilot default", () => {
    expect(
      resolveLocale({ userLocale: "es", clubDefaultLocale: "es", stored: "en" }),
    ).toBe("en");
  });

  it("resolves user over club when nothing stored", () => {
    expect(resolveLocale({ userLocale: "en", clubDefaultLocale: "es" })).toBe("en");
  });

  it("defaults to Spanish without stored locale, account, or browser", () => {
    expect(resolveLocale({})).toBe("es");
  });

  it("resolves Accept-Language when nothing stored or on account", () => {
    expect(resolveLocale({ acceptLanguage: "en-US,en;q=0.9" })).toBe("en");
  });

  it("PATCHes pilot locale when session exists", async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true });
    vi.stubGlobal("fetch", fetchMock);
    initI18n({ userLocale: "es" });
    await setLocale("en");
    expect(getLocale()).toBe("en");
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/club/me/locale",
      expect.objectContaining({ method: "PATCH", body: JSON.stringify({ locale: "en" }) }),
    );
    vi.unstubAllGlobals();
  });

  it("does not PATCH when anonymous", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    initI18n({ userLocale: null });
    await setLocale("en");
    expect(fetchMock).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("maps api error slugs", () => {
    initI18n();
    expect(apiErrorMessage("unauthorized")).toBe(t("api.unauthorized"));
  });

  it("applies data-i18n-aria and data-i18n-alt attributes", () => {
    initI18n({ stored: "es" });
    const wrap = document.createElement("div");
    wrap.innerHTML = `
      <section data-i18n-aria="kiosk.liveMapAria"></section>
      <img data-i18n-alt="kiosk.qrAlt" />
    `;
    applyI18nDocument(wrap);
    expect(wrap.querySelector("section")?.getAttribute("aria-label")).toBe(
      t("kiosk.liveMapAria"),
    );
    expect(wrap.querySelector("img")?.alt).toBe(t("kiosk.qrAlt"));
  });
});