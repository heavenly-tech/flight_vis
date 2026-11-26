import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initI18n, t } from "../i18n/index";
import { bootAccountPage } from "./accountPage";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

function mountAccountDom(): void {
  document.body.innerHTML = `
    <main class="auth-card">
      <div id="lang-slot" class="lang-bar"></div>
      <h1 data-i18n="account.password.title">Cuenta</h1>
      <p id="email-line" class="muted"></p>
      <form id="password-form">
        <div id="current-wrap" class="field">
          <label id="current-label" for="current" data-i18n="account.password.current">Contraseña actual</label>
          <input id="current" name="current_password" type="password" />
        </div>
        <div class="field">
          <label for="new-password" data-i18n="account.password.new">Nueva contraseña</label>
          <input id="new-password" name="password" type="password" required minlength="8" />
        </div>
        <button type="submit" class="btn-primary" data-i18n="account.password.submit">Guardar contraseña</button>
        <p id="msg" class="status-msg" role="status" aria-live="polite"></p>
      </form>
      <p class="muted"><a href="/account-clubs.html" data-i18n="club.login.misClubs">Mis clubes</a></p>
    </main>
  `;
}

describe("accountPage password save feedback", () => {
  beforeEach(() => {
    localStorage.clear();
    document.cookie = "fv_locale=; Path=/; Max-Age=0";
    mountAccountDom();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("shows Contraseña guardada. (not Sesión iniciada.) after saving password in es-CL", async () => {
    initI18n({ stored: "es" });
    localStorage.setItem("fv_locale", "es");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url === "/api/club/i18n/context") {
          return new Response(JSON.stringify({ resolvedLocale: "es" }), { status: 200 });
        }
        if (url === "/api/account/me") {
          return new Response(
            JSON.stringify({
              account: { email: "admin@demo.test" },
              skipCurrentPassword: false,
            }),
            { status: 200 },
          );
        }
        if (url === "/api/account/password") {
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    await bootAccountPage();
    expect(document.getElementById("email-line")?.textContent).toBe("admin@demo.test");

    document.querySelector<HTMLInputElement>("#current")!.value = "password12";
    document.querySelector<HTMLInputElement>("#new-password")!.value = "newpassword99";
    document
      .querySelector<HTMLFormElement>("#password-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));

    const msg = document.getElementById("msg")!;
    await vi.waitFor(() => expect(msg.textContent).toBe("Contraseña guardada."));
    expect(msg.textContent).not.toBe(t("club.login.success"));
  });

  it("shows Password saved. in English and account.html links back to Mis clubes", async () => {
    initI18n({ stored: "en" });
    localStorage.setItem("fv_locale", "en");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url === "/api/club/i18n/context") {
          return new Response(JSON.stringify({ resolvedLocale: "en" }), { status: 200 });
        }
        if (url === "/api/account/me") {
          return new Response(
            JSON.stringify({
              account: { email: "admin@demo.test" },
              skipCurrentPassword: true,
            }),
            { status: 200 },
          );
        }
        if (url === "/api/account/password") {
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    await bootAccountPage();
    document.querySelector<HTMLInputElement>("#new-password")!.value = "newpassword99";
    document
      .querySelector<HTMLFormElement>("#password-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));

    const msg = document.getElementById("msg")!;
    await vi.waitFor(() => expect(msg.textContent).toBe("Password saved."));
    expect(msg.textContent).not.toBe("Signed in.");

    const accountHtml = fs.readFileSync(path.join(root, "account.html"), "utf8");
    expect(accountHtml).toContain('href="/account-clubs.html"');
  });
});
