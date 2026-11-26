import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

const rootHtmlFiles = fs
  .readdirSync(root)
  .filter((name) => name.endsWith(".html"))
  .sort();

const EN_ONLY_ARIA_OR_ALT = [
  "Live club map",
  "Check-in QR",
  "QR preview",
  "Toggle language",
  "Altitude profile and trim",
  "flight_vis home",
  "Live and language",
  "qr",
];

describe("root HTML accessibility and shared styling audit", () => {
  it("every root HTML file declares an appropriate es-CL or en lang attribute", () => {
    expect(rootHtmlFiles.length).toBeGreaterThan(10);
    for (const file of rootHtmlFiles) {
      const html = fs.readFileSync(path.join(root, file), "utf8");
      const dom = new JSDOM(html);
      const lang = dom.window.document.documentElement.getAttribute("lang") ?? "";
      if (file === "e2e-live.html") {
        expect(lang.startsWith("en"), `${file} lang="${lang}"`).toBe(true);
      } else {
        expect(lang.startsWith("es-CL"), `${file} lang="${lang}"`).toBe(true);
      }
    }
  });

  it("every interactive form control in root HTML files has an associated label", () => {
    const unlabeled: string[] = [];
    for (const file of rootHtmlFiles) {
      const html = fs.readFileSync(path.join(root, file), "utf8");
      const dom = new JSDOM(html);
      const doc = dom.window.document;
      const controls = doc.querySelectorAll("input, select, textarea");
      for (const el of controls) {
        const tag = el.tagName.toLowerCase();
        const type = (el.getAttribute("type") ?? "").toLowerCase();
        if (tag === "input" && ["hidden", "submit", "button", "reset", "image"].includes(type)) {
          continue;
        }
        if (el.getAttribute("aria-hidden") === "true") {
          continue;
        }
        const id = el.getAttribute("id");
        const hasForLabel = Boolean(id && doc.querySelector(`label[for="${id}"]`));
        const hasWrappingLabel = Boolean(el.closest("label"));
        const hasAriaLabel = Boolean(el.getAttribute("aria-label")?.trim());
        const hasAriaLabelledBy = Boolean(el.getAttribute("aria-labelledby")?.trim());
        const hasI18nAria = Boolean(el.getAttribute("data-i18n-aria")?.trim());
        if (!hasForLabel && !hasWrappingLabel && !hasAriaLabel && !hasAriaLabelledBy && !hasI18nAria) {
          unlabeled.push(`${file}: ${el.outerHTML}`);
        }
      }
    }
    expect(unlabeled).toEqual([]);
  });

  it("no root HTML file retains known English-only aria-label or alt literals", () => {
    const hits: string[] = [];
    for (const file of rootHtmlFiles) {
      const html = fs.readFileSync(path.join(root, file), "utf8");
      const dom = new JSDOM(html);
      const doc = dom.window.document;
      for (const el of doc.querySelectorAll("[aria-label], [alt]")) {
        const aria = el.getAttribute("aria-label")?.trim();
        const alt = el.getAttribute("alt")?.trim();
        if (aria && EN_ONLY_ARIA_OR_ALT.includes(aria)) {
          hits.push(`${file}: aria-label="${aria}"`);
        }
        if (alt && EN_ONLY_ARIA_OR_ALT.includes(alt)) {
          hits.push(`${file}: alt="${alt}"`);
        }
      }
    }
    expect(hits).toEqual([]);
  });

  it("no root HTML file uses #ddd or #ccc light borders and user pages include the shared stylesheet", () => {
    const mainTs = fs.readFileSync(path.join(root, "src", "main.ts"), "utf8");
    const mainImportsStyle = mainTs.includes("./style.css");
    expect(mainImportsStyle).toBe(true);

    const lightBorders: string[] = [];
    const missingStyle: string[] = [];

    for (const file of rootHtmlFiles) {
      const html = fs.readFileSync(path.join(root, file), "utf8");
      if (/#(?:ddd|ccc)\b/i.test(html)) {
        lightBorders.push(file);
      }
      if (file === "e2e-live.html") continue;
      if (file === "index.html") {
        if (!html.includes("/src/main.ts") || !mainImportsStyle) {
          missingStyle.push(file);
        }
        continue;
      }
      if (!html.includes('href="/src/style.css"')) {
        missingStyle.push(file);
      }
    }

    expect(lightBorders).toEqual([]);
    expect(missingStyle).toEqual([]);
  });

  it("status and error elements declare live-region roles", () => {
    const statusSelectorsByFile: Record<string, string[]> = {
      "index.html": ["#status", "#live-status", "#rename-warn", "#admin-qr-status"],
      "login.html": ["#form-msg"],
      "login-legacy.html": ["#err", "#locked"],
      "signup.html": ["#msg"],
      "club-register.html": ["#msg"],
      "club-signup.html": ["#msg"],
      "account.html": ["#msg"],
      "account-clubs.html": ["#join-msg", "#clubs-msg"],
      "app.html": ["#status"],
      "club-admin-kiosk-layout.html": ["#save-status"],
      "club-admin-map.html": [
        "#club-locale-msg",
        "#club-slug-msg",
        "#club-base-msg",
        "#club-delete-msg",
        "#tails-msg",
        "#live-empty",
      ],
      "club-admin-checkin-qr.html": ["#status"],
      "club-admin-pilots.html": ["#invite-status"],
      "club-screen.html": ["#status-bar"],
      "public-map.html": ["#empty-state", "#map-error"],
      "share-flight.html": ["#status"],
    };

    const missingLive: string[] = [];
    for (const [file, selectors] of Object.entries(statusSelectorsByFile)) {
      const html = fs.readFileSync(path.join(root, file), "utf8");
      const dom = new JSDOM(html);
      const doc = dom.window.document;
      for (const sel of selectors) {
        const el = doc.querySelector(sel);
        if (!el) {
          missingLive.push(`${file}: missing element ${sel}`);
          continue;
        }
        const role = el.getAttribute("role");
        const ariaLive = el.getAttribute("aria-live");
        const ok = role === "alert" || (role === "status" && ariaLive === "polite");
        if (!ok) {
          missingLive.push(`${file} ${sel}: role=${role} aria-live=${ariaLive}`);
        }
      }
    }
    expect(missingLive).toEqual([]);
  });

  it("kiosk layout editor merges save actions into one Guardar button and includes keyboard controls", () => {
    const html = fs.readFileSync(path.join(root, "club-admin-kiosk-layout.html"), "utf8");
    const dom = new JSDOM(html);
    const doc = dom.window.document;
    expect(doc.querySelector("#btn-save")).not.toBeNull();
    expect(doc.querySelector("#btn-save-visible")).toBeNull();
    expect(doc.querySelector("#qr-x")).not.toBeNull();
    expect(doc.querySelector("#qr-y")).not.toBeNull();
    expect(doc.querySelector("#qr-size")).not.toBeNull();
    expect(html).not.toContain("Arrastre");
  });

  it("shared stylesheet defines dark-theme link color and visually-hidden utility", () => {
    const styleCss = fs.readFileSync(path.join(root, "src", "style.css"), "utf8");
    const sharedCss = fs.readFileSync(path.join(root, "src", "uiShared.css"), "utf8");
    expect(styleCss).toContain('@import "./uiShared.css"');
    expect(sharedCss).toContain("#67e8f9");
    expect(sharedCss).toContain(".visually-hidden");
    expect(sharedCss).toContain(".auth-card");
    expect(sharedCss).toContain(".field");
    expect(sharedCss).toContain(".btn-primary");
    expect(sharedCss).toContain(".btn-secondary");
    expect(sharedCss).toContain(".lang-bar");
  });
});
