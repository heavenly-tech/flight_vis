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

  it("keeps magic link as default login method and styles both signup links secondary", () => {
    document.body.innerHTML = loadHtmlBody("login.html");
    initI18n({ stored: "es" });
    applyI18nDocument(document);

    // Magic link remains the default login method (password stays hidden until toggled).
    const form = document.querySelector<HTMLFormElement>("#login-form")!;
    expect(form.getAttribute("action")).toBe("/api/account/magic-link");
    const loginLead = document.querySelector<HTMLParagraphElement>("#login-lead")!;
    expect(loginLead).not.toBeNull();
    expect(loginLead.dataset.i18n).toBe("entrar.magicLead");
    expect(loginLead.textContent?.trim()).toBe(
      "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.",
    );
    const submitMagic = document.querySelector<HTMLButtonElement>("#submit-magic")!;
    const submitPassword = document.querySelector<HTMLButtonElement>("#submit-password")!;
    const passwordReveal = document.querySelector<HTMLElement>("#password-reveal")!;
    const usePassword = document.querySelector<HTMLAnchorElement>("#use-password")!;
    const legacyDetails = document.querySelector<HTMLDetailsElement>("#legacy-details")!;
    expect(submitMagic.hidden).toBe(false);
    expect(submitMagic.textContent?.trim()).toBe("Enviar enlace");
    expect(passwordReveal.hidden).toBe(true);
    expect(submitPassword.hidden).toBe(true);
    expect(usePassword.hidden).toBe(false);
    expect(legacyDetails.hidden).toBe(true);
    expect(legacyDetails.hasAttribute("open")).toBe(false);
    expect(submitMagic.compareDocumentPosition(usePassword) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    // Signup CTA hierarchy: both Registrar piloto (/signup.html) and Registrar club (/club-signup.html)
    // use secondary styling so the main login submit stays the single primary CTA.
    const registerPilot = document.querySelector<HTMLAnchorElement>("#register-pilot")!;
    const registerClub = document.querySelector<HTMLAnchorElement>("#register-club")!;
    expect(registerPilot).not.toBeNull();
    expect(registerClub).not.toBeNull();
    expect(registerPilot.textContent?.trim()).toBe("Registrar piloto");
    expect(registerClub.textContent?.trim()).toBe("Registrar club");
    expect(registerPilot.classList.contains("secondary")).toBe(true);
    expect(registerPilot.classList.contains("primary")).toBe(false);
    expect(registerClub.classList.contains("secondary")).toBe(true);
    expect(registerClub.classList.contains("primary")).toBe(false);
    expect(
      registerPilot.compareDocumentPosition(registerClub) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(registerPilot.getAttribute("href")).toBe("/signup.html");
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

  it("restores signup.html and keeps signup links exclusively on login.html", () => {
    expect(fs.existsSync(path.join(root, "signup.html"))).toBe(true);

    const htmlFiles = fs
      .readdirSync(root)
      .filter((f) => f.endsWith(".html"))
      .sort();
    expect(htmlFiles.length).toBeGreaterThan(10);

    const forbiddenHref =
      /href=["']\/(?:signup(?:\.html)?|club-register\.html|club-signup(?:\.html)?)(?:\?[^"']*)?["']/i;

    for (const file of htmlFiles) {
      if (file === "login.html") continue;
      const content = fs.readFileSync(path.join(root, file), "utf8");
      expect(
        content,
        `${file} must not link to signup.html, club-register.html, or club-signup.html`,
      ).not.toMatch(forbiddenHref);
    }

    // Main site (index.html) shows only Iniciar sesión (#btn-login), no signup CTAs.
    document.body.innerHTML = loadHtmlBody("index.html");
    initI18n({ stored: "es" });
    applyI18nDocument(document);
    const loginBtn = document.querySelector<HTMLAnchorElement>("#btn-login");
    expect(loginBtn).not.toBeNull();
    expect(loginBtn?.getAttribute("href")).toBe("/login");
    expect(loginBtn?.textContent?.trim()).toBe("Iniciar sesión");
    expect(document.querySelector("#register-pilot")).toBeNull();
    expect(document.querySelector("#register-club")).toBeNull();
  });
});
