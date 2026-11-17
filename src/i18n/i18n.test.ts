import { describe, expect, it } from "vitest";
import { flattenCatalog, bundledLocaleIds, CANONICAL_LOCALE } from "./catalog";
import { initI18n, t, resolveLocale, apiErrorMessage } from "./index";

describe("i18n catalogs", () => {
  it("bundled locales share canonical keys", () => {
    const canon = Object.keys(flattenCatalog(CANONICAL_LOCALE)).sort();
    for (const id of bundledLocaleIds()) {
      const keys = Object.keys(flattenCatalog(id)).sort();
      expect(keys).toEqual(canon);
    }
  });

  it("defaults to Spanish", () => {
    initI18n();
    expect(t("club.login.title")).toContain("sesión");
  });

  it("resolves user over club", () => {
    expect(
      resolveLocale({ userLocale: "en", clubDefaultLocale: "es", stored: "es" }),
    ).toBe("en");
  });

  it("maps api error slugs", () => {
    initI18n();
    expect(apiErrorMessage("unauthorized")).toBe(t("api.unauthorized"));
  });
});