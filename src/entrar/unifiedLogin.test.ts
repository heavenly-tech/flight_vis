import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { initI18n, t } from "../i18n/index";
import { bootUnifiedLogin, MAGIC_LEAD_KEY, PASSWORD_LEAD_KEY } from "./unifiedLogin";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function loadHtmlBody(fileName: string): string {
  const html = fs.readFileSync(path.join(root, fileName), "utf8");
  const match = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);
  return match ? match[1] : html;
}

describe("unifiedLogin password mode lead and collapsed legacy details", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
    document.body.innerHTML = loadHtmlBody("login.html");
    window.history.replaceState({}, "", "/login");
  });

  afterEach(() => {
    document.body.innerHTML = "";
    window.history.replaceState({}, "", "/");
  });

  it("toggles #login-lead between magicLead and passwordLead and keeps #legacy-details collapsed", () => {
    bootUnifiedLogin({ translate: t, skipAsyncBoot: true });

    const lead = document.querySelector<HTMLParagraphElement>("#login-lead")!;
    const legacy = document.querySelector<HTMLDetailsElement>("#legacy-details")!;
    const usePw = document.querySelector<HTMLAnchorElement>("#use-password")!;
    const backMagic = document.querySelector<HTMLAnchorElement>("#back-magic")!;

    expect(lead.dataset.i18n).toBe(MAGIC_LEAD_KEY);
    expect(lead.textContent).toBe("Ingresa tu correo y te enviaremos un enlace para iniciar sesión.");
    expect(legacy.hidden).toBe(true);
    expect(legacy.hasAttribute("open")).toBe(false);

    usePw.click();
    expect(lead.dataset.i18n).toBe(PASSWORD_LEAD_KEY);
    expect(lead.textContent).toBe("Inicia sesión con tu contraseña.");
    expect(legacy.hidden).toBe(false);
    expect(legacy.hasAttribute("open")).toBe(false);

    backMagic.click();
    expect(lead.dataset.i18n).toBe(MAGIC_LEAD_KEY);
    expect(lead.textContent).toBe("Ingresa tu correo y te enviaremos un enlace para iniciar sesión.");
    expect(legacy.hidden).toBe(true);
    expect(legacy.hasAttribute("open")).toBe(false);
  });

  it("initializes directly in ?mode=password with password lead and collapsed #legacy-details", () => {
    window.history.replaceState({}, "", "/login?mode=password");
    // Simulate a stale open attribute before boot to ensure boot removes it.
    document.querySelector("#legacy-details")?.setAttribute("open", "");
    initI18n({ stored: "en" });

    bootUnifiedLogin({ translate: t, skipAsyncBoot: true });

    const lead = document.querySelector<HTMLParagraphElement>("#login-lead")!;
    const legacy = document.querySelector<HTMLDetailsElement>("#legacy-details")!;

    expect(lead.dataset.i18n).toBe(PASSWORD_LEAD_KEY);
    expect(lead.textContent).toBe("Sign in with your password.");
    expect(legacy.hidden).toBe(false);
    expect(legacy.hasAttribute("open")).toBe(false);
  });
});
