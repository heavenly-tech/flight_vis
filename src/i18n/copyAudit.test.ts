import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import {
  apiErrorMessage,
  applyI18nDocument,
  initI18n,
  setLocale,
  t,
} from "./index";
import { friendlyErrorMessage } from "./errorMessages";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function collectLeafValues(value: unknown, prefix = ""): Array<{ key: string; text: string }> {
  if (typeof value === "string") return [{ key: prefix, text: value }];
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return Object.entries(value).flatMap(([k, v]) =>
      k === "_meta" ? [] : collectLeafValues(v, prefix ? `${prefix}.${k}` : k),
    );
  }
  return [];
}

describe("es.json and en.json copy quality audit", () => {
  const esRaw = fs.readFileSync(path.join(root, "locales", "es.json"), "utf8");
  const enRaw = fs.readFileSync(path.join(root, "locales", "en.json"), "utf8");
  const esEntries = collectLeafValues(JSON.parse(esRaw));
  const enEntries = collectLeafValues(JSON.parse(enRaw));

  it("es.json contains none of the banned copy substrings", () => {
    const bannedSubstrings = [
      "Arrastre",
      "Introduce ",
      " clubs",
      "/api/",
      "FLIGHT_VIS_PASSWORD",
      "SSE",
      "{{",
      "fixes",
      "Slug",
      "Traccar",
      "OsmAnd",
      "/login",
      "(instrucciones de instalación)",
      "QR kiosk",
      "Mapa admin",
      "QR check-in",
      "APK legacy",
    ];

    const violations: string[] = [];
    for (const { key, text } of esEntries) {
      for (const bad of bannedSubstrings) {
        if (text.includes(bad)) {
          violations.push(`${key}: contains "${bad}" -> ${text}`);
        }
      }
      // Check whole-word anglicisms outside {slug} placeholders
      const withoutVars = text.replace(/\{[^}]+\}/g, "");
      if (/\btracks?\b/i.test(withoutVars)) {
        violations.push(`${key}: contains track(s) -> ${text}`);
      }
      if (/\bStaff\b/i.test(withoutVars)) {
        violations.push(`${key}: contains staff -> ${text}`);
      }
      if (/\bStats\b/.test(withoutVars)) {
        violations.push(`${key}: contains Stats -> ${text}`);
      }
      if (/\bslug\b/i.test(withoutVars)) {
        violations.push(`${key}: contains slug -> ${text}`);
      }
    }
    expect(violations).toEqual([]);
  });

  it("en.json contains no leaked internal paths, env vars, or stub text", () => {
    const bannedEn = [
      "/api/",
      "FLIGHT_VIS_PASSWORD",
      "SSE",
      "Traccar",
      "OsmAnd",
      "/login",
      "{{",
      "(sideload instructions)",
    ];
    const violations: string[] = [];
    for (const { key, text } of enEntries) {
      for (const bad of bannedEn) {
        if (text.includes(bad)) {
          violations.push(`${key}: contains "${bad}" -> ${text}`);
        }
      }
    }
    expect(violations).toEqual([]);
  });

  it("localizes HUD labels and summary max in Spanish and English", () => {
    initI18n({ stored: "es" });
    expect(t("globe.hudAlt")).toBe("ALT");
    expect(t("globe.hudSpeed")).toBe("VEL");
    expect(t("globe.hudVario")).toBe("VARIO");
    expect(t("globe.hudTime")).toBe("TIEMPO");
    expect(t("globe.hudDist")).toBe("DIST");
    expect(t("globe.hudUtc")).toBe("UTC");
    expect(t("globe.hudAcc")).toBe("PREC");
    expect(t("globe.stats")).toBe("Resumen");
    expect(t("globe.summaryMax")).toBe("máx.");
    expect(t("club.login.submit")).toBe("Iniciar sesión");

    initI18n({ stored: "en" });
    expect(t("globe.hudSpeed")).toBe("SPD");
    expect(t("globe.hudTime")).toBe("TIME");
    expect(t("globe.hudAcc")).toBe("ACC");
    expect(t("globe.stats")).toBe("Stats");
    expect(t("globe.summaryMax")).toBe("max");

    const indexHtml = fs.readFileSync(path.join(root, "index.html"), "utf8");
    expect(indexHtml).toContain('data-i18n="globe.hudAlt"');
    expect(indexHtml).toContain('data-i18n="globe.hudSpeed"');
    expect(indexHtml).toContain('data-i18n="globe.hudVario"');
    expect(indexHtml).toContain('data-i18n="globe.hudTime"');
    expect(indexHtml).toContain('data-i18n="globe.hudDist"');
    expect(indexHtml).toContain('data-i18n="globe.hudUtc"');
    expect(indexHtml).toContain('data-i18n="globe.hudAcc"');
  });

  it("updates document.documentElement.lang and localizes data-i18n-aria and data-i18n-alt on locale switch", async () => {
    document.body.innerHTML = `
      <section id="map" data-i18n-aria="kiosk.mapAria"></section>
      <img id="qr" data-i18n-alt="kiosk.qrAlt" alt="" />
    `;
    initI18n({ stored: "es" });
    applyI18nDocument();
    expect(document.documentElement.lang).toBe("es-CL");
    expect(document.getElementById("map")?.getAttribute("aria-label")).toBe("Mapa del club en vivo");
    expect((document.getElementById("qr") as HTMLImageElement).alt).toBe("Código QR de registro");

    await setLocale("en", { persistUser: false });
    expect(document.documentElement.lang).toBe("en");
    expect(document.getElementById("map")?.getAttribute("aria-label")).toBe("Live club map");
    expect((document.getElementById("qr") as HTMLImageElement).alt).toBe("Check-in QR");

    await setLocale("es", { persistUser: false });
    expect(document.documentElement.lang).toBe("es-CL");
  });

  it("maps unknown API/runtime errors to friendly localized text without leaking raw codes", () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    initI18n({ stored: "es" });
    expect(apiErrorMessage("invalid_internal_sql_code_42P01")).toBe(t("api.genericError"));
    expect(apiErrorMessage("locked")).toBe(t("globe.uploadLocked"));
    expect(friendlyErrorMessage(new Error("Upload is locked until FLIGHT_VIS_PASSWORD is set."))).toBe(
      t("globe.uploadLocked"),
    );
    expect(friendlyErrorMessage(new Error("GPX file is not valid XML."))).toBe(
      t("globe.errorInvalidGpx"),
    );
    expect(
      friendlyErrorMessage(new Error('{"error":"unexpected_debug_id_999"}'), "globe.couldNotSave"),
    ).toBe(t("globe.couldNotSave"));
    warnSpy.mockRestore();
  });
});
