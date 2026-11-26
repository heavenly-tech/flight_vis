import { safeNextPath } from "../club/safeNext";

export const MAGIC_LEAD_KEY = "entrar.magicLead";
export const PASSWORD_LEAD_KEY = "entrar.passwordLead";
const MAGIC_LEAD_FALLBACK_ES = "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.";
const PASSWORD_LEAD_FALLBACK_ES = "Inicia sesión con tu contraseña.";

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
  }
}

export interface BootUnifiedLoginOptions {
  translate?: (key: string) => string;
  skipAsyncBoot?: boolean;
}

export function bootUnifiedLogin(opts: BootUnifiedLoginOptions = {}): void {
  const params = new URLSearchParams(location.search);
  let nextAfterLogin = safeNextPath(params.get("next") ?? "/");
  const serverPasswordMode = params.get("mode") === "password";

  const loginForm = document.querySelector<HTMLFormElement>("#login-form");
  const loginLead = document.getElementById("login-lead");
  const nextInput = document.querySelector<HTMLInputElement>("#next");
  const passwordReveal = document.getElementById("password-reveal");
  const passwordInput = document.querySelector<HTMLInputElement>("#password");
  const submitMagic = document.getElementById("submit-magic");
  const submitPassword = document.getElementById("submit-password");
  const formMsg = document.querySelector<HTMLParagraphElement>("#form-msg");
  const emailInput = document.querySelector<HTMLInputElement>("#email");
  const usePassword = document.getElementById("use-password");
  const backMagic = document.getElementById("back-magic");
  const legacyDetails = document.getElementById("legacy-details");

  let passwordMode = serverPasswordMode;
  let translateFn: ((key: string) => string) | null = opts.translate ?? null;

  window.__fvUnifiedLoginReady = true;

  function syncNext(): void {
    if (nextInput) nextInput.value = nextAfterLogin;
  }

  function linkWithNext(href: string, password?: boolean): string {
    const q = new URLSearchParams();
    if (password) q.set("mode", "password");
    if (nextAfterLogin !== "/") q.set("next", nextAfterLogin);
    const qs = q.toString();
    const base = href.split("?")[0];
    return qs ? `${base}?${qs}` : base;
  }

  function setPasswordMode(on: boolean): void {
    passwordMode = on;
    const leadKey = on ? PASSWORD_LEAD_KEY : MAGIC_LEAD_KEY;
    if (loginLead) {
      const prevKey = loginLead.dataset.i18n;
      loginLead.dataset.i18n = leadKey;
      if (translateFn) {
        loginLead.textContent = translateFn(leadKey);
      } else if (prevKey !== leadKey) {
        loginLead.textContent = on ? PASSWORD_LEAD_FALLBACK_ES : MAGIC_LEAD_FALLBACK_ES;
      }
    }
    if (on) {
      passwordReveal?.removeAttribute("hidden");
      submitMagic?.setAttribute("hidden", "");
      submitPassword?.removeAttribute("hidden");
      usePassword?.setAttribute("hidden", "");
      backMagic?.removeAttribute("hidden");
      legacyDetails?.removeAttribute("hidden");
      legacyDetails?.removeAttribute("open");
    } else {
      passwordReveal?.setAttribute("hidden", "");
      submitMagic?.removeAttribute("hidden");
      submitPassword?.setAttribute("hidden", "");
      usePassword?.removeAttribute("hidden");
      backMagic?.setAttribute("hidden", "");
      legacyDetails?.setAttribute("hidden", "");
      legacyDetails?.removeAttribute("open");
    }
    if (loginForm) {
      loginForm.action = on ? "/api/account/login" : "/api/account/magic-link";
    }
    if (passwordInput) {
      if (on) passwordInput.setAttribute("required", "");
      else passwordInput.removeAttribute("required");
    }
    if (usePassword instanceof HTMLAnchorElement) {
      usePassword.href = linkWithNext("/login", true);
    }
    if (backMagic instanceof HTMLAnchorElement) {
      backMagic.href = linkWithNext("/login", false);
    }
  }

  syncNext();
  setPasswordMode(serverPasswordMode);

  usePassword?.addEventListener("click", (ev) => {
    ev.preventDefault();
    setPasswordMode(true);
    passwordInput?.focus();
  });

  backMagic?.addEventListener("click", (ev) => {
    if (!serverPasswordMode) {
      ev.preventDefault();
      setPasswordMode(false);
      if (formMsg) formMsg.textContent = "";
    }
  });

  loginForm?.addEventListener("submit", (ev) => {
    const sub = ev.submitter;
    const legacy =
      sub instanceof HTMLButtonElement && sub.id === "legacy-submit";
    if (legacy || passwordMode) {
      const button = sub instanceof HTMLButtonElement ? sub : submitPassword;
      if (button instanceof HTMLButtonElement) button.disabled = true;
      return;
    }
    ev.preventDefault();
    void (async () => {
      const email = emailInput?.value.trim() ?? "";
      if (!email) return;
      const { t, getLocale } = await import("../i18n/index");
      const res = await fetch("/api/account/magic-link", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Accept: "application/json",
        },
        credentials: "same-origin",
        body: JSON.stringify({ email, locale: getLocale() }),
      });
      if (formMsg) {
        formMsg.setAttribute("role", res.ok ? "status" : "alert");
        formMsg.textContent = res.ok ? t("club.login.magicSent") : t("club.login.failed");
      }
    })();
  });

  if (opts.skipAsyncBoot) return;

  void (async () => {
    const [{ bootI18n, applyI18nDocument }, { t }] = await Promise.all([
      import("../i18n/boot"),
      import("../i18n/index"),
    ]);
    await bootI18n({ switcher: "#lang-slot", persistUser: false });
    translateFn = t;
    applyI18nDocument(document);
    nextAfterLogin = safeNextPath(
      new URLSearchParams(location.search).get("next") ?? "/",
    );
    syncNext();
    if (usePassword instanceof HTMLAnchorElement) {
      usePassword.href = linkWithNext("/login", true);
    }
    if (backMagic instanceof HTMLAnchorElement) {
      backMagic.href = linkWithNext("/login", false);
    }
    if (params.get("magic") === "1" && formMsg) {
      formMsg.textContent = t("club.login.magicSent");
    }
    const err = params.get("e") === "1";
    if (err) {
      const { apiErrorMessage } = await import("../i18n/index");
      if (formMsg) {
        formMsg.setAttribute("role", "alert");
        formMsg.textContent = apiErrorMessage("unauthorized");
      }
    }
    const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
    if (meRes.ok) {
      location.replace(
        `/account-clubs.html?next=${encodeURIComponent(nextAfterLogin)}`,
      );
    }
  })();
}

if (typeof document !== "undefined" && document.getElementById("login-form")) {
  bootUnifiedLogin();
}
