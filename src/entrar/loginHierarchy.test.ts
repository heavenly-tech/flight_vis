import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { applyI18nDocument, initI18n } from "../i18n/index";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function loadHtmlBody(fileName: string): string {
  const html = fs.readFileSync(path.join(root, fileName), "utf8");
  const match = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);
  return match ? match[1] : html;
}

describe("login and pilot signup hierarchy", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("keeps magic link as default login method and promotes Registrar piloto over Registrar club", () => {
    document.body.innerHTML = loadHtmlBody("login.html");
    initI18n({ stored: "es" });
    applyI18nDocument(document);

    // Magic link remains the default login method (password stays hidden until toggled).
    const form = document.querySelector<HTMLFormElement>("#login-form")!;
    expect(form.getAttribute("action")).toBe("/api/account/magic-link");
    const submitMagic = document.querySelector<HTMLButtonElement>("#submit-magic")!;
    const submitPassword = document.querySelector<HTMLButtonElement>("#submit-password")!;
    const passwordReveal = document.querySelector<HTMLElement>("#password-reveal")!;
    const usePassword = document.querySelector<HTMLAnchorElement>("#use-password")!;
    expect(submitMagic.hidden).toBe(false);
    expect(submitMagic.textContent?.trim()).toBe("Enviar enlace");
    expect(passwordReveal.hidden).toBe(true);
    expect(submitPassword.hidden).toBe(true);
    expect(usePassword.hidden).toBe(false);
    expect(submitMagic.compareDocumentPosition(usePassword) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    // Signup CTA hierarchy: Registrar piloto is primary and first; Registrar club is secondary and second.
    const registerPilot = document.querySelector<HTMLAnchorElement>("#register-pilot")!;
    const registerClub = document.querySelector<HTMLAnchorElement>("#register-club")!;
    expect(registerPilot).not.toBeNull();
    expect(registerClub).not.toBeNull();
    expect(registerPilot.textContent?.trim()).toBe("Registrar piloto");
    expect(registerClub.textContent?.trim()).toBe("Registrar club");
    expect(registerPilot.classList.contains("primary")).toBe(true);
    expect(registerPilot.classList.contains("secondary")).toBe(false);
    expect(registerClub.classList.contains("secondary")).toBe(true);
    expect(registerClub.classList.contains("primary")).toBe(false);
    expect(
      registerPilot.compareDocumentPosition(registerClub) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(registerPilot.getAttribute("href")).toBe("/club-register.html");
    expect(registerClub.getAttribute("href")).toBe("/club-signup.html");

    const ordered = [...document.querySelectorAll("#submit-magic, #use-password, #register-pilot, #register-club")].map(
      (el) => el.id,
    );
    expect(ordered).toEqual(["submit-magic", "use-password", "register-pilot", "register-club"]);

    // Verify English locale strings also stay aligned.
    initI18n({ stored: "en" });
    applyI18nDocument(document);
    expect(submitMagic.textContent?.trim()).toBe("Send sign-in link");
    expect(registerPilot.textContent?.trim()).toBe("Register as pilot");
    expect(registerClub.textContent?.trim()).toBe("Register club");
  });

  it("unifies all pilot signup entry points onto /club-register.html with no duplicate signup.html", () => {
    expect(fs.existsSync(path.join(root, "signup.html"))).toBe(false);

    const htmlFiles = fs.readdirSync(root).filter((f) => f.endsWith(".html"));
    for (const file of htmlFiles) {
      const content = fs.readFileSync(path.join(root, file), "utf8");
      expect(content, `${file} must not link to removed /signup.html`).not.toMatch(
        /href=["']\/signup(?:\.html)?(?:\?[^"']*)?["']/,
      );
    }

    for (const file of ["login.html", "club-signup.html", "club-checkin-landing.html"]) {
      const content = fs.readFileSync(path.join(root, file), "utf8");
      expect(content, `${file} should link to /club-register.html`).toContain(
        'href="/club-register.html"',
      );
    }
  });
});
