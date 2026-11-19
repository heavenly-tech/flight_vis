import { safeNextPath } from "../club/safeNext";

const params = new URLSearchParams(location.search);
let nextAfterLogin = safeNextPath(params.get("next") ?? "/");

const form = document.querySelector<HTMLFormElement>("#login-form");
const msg = document.querySelector<HTMLParagraphElement>("#msg");
const nextInput = document.querySelector<HTMLInputElement>("#next");

if (nextInput) nextInput.value = nextAfterLogin;

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
  }
}

window.__fvUnifiedLoginReady = true;

if (form) {
  form.addEventListener(
    "submit",
    async (e) => {
      e.preventDefault();
      const body = Object.fromEntries(new FormData(form));
      const email = String(body.email ?? "").trim();
      const password = String(body.password ?? "");
      if (!email) return;
      const { t, apiErrorMessage } = await import("../i18n/index");
      if (!password) {
        if (msg) msg.textContent = t("club.login.passwordOrMagic");
        return;
      }
      const res = await fetch("/api/account/login", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Accept: "application/json",
        },
        credentials: "same-origin",
        body: JSON.stringify({ email, password, next: nextAfterLogin }),
      });
      if (!res.ok) {
        const err = (await res.json().catch(() => ({}))).error ?? "unauthorized";
        if (msg) msg.textContent = apiErrorMessage(String(err));
        return;
      }
      location.replace(
        `/account-clubs.html?next=${encodeURIComponent(nextAfterLogin)}`,
      );
    },
    { capture: true },
  );
}

document.getElementById("magic-btn")?.addEventListener("click", async () => {
  if (!form) return;
  const email = (form.querySelector('input[name="email"]') as HTMLInputElement).value.trim();
  if (!email) return;
  const { t, getLocale } = await import("../i18n/index");
  const res = await fetch("/api/account/magic-link", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ email, locale: getLocale() }),
  });
  if (msg) {
    msg.textContent = res.ok ? t("club.login.magicSent") : t("club.login.failed");
  }
});

void (async () => {
  const { bootI18n, applyI18nDocument } = await import("../i18n/boot");
  await bootI18n({ switcher: "#lang-slot", persistUser: false });
  applyI18nDocument(document);
  nextAfterLogin = safeNextPath(
    new URLSearchParams(location.search).get("next") ?? "/",
  );
  if (nextInput) nextInput.value = nextAfterLogin;
  if (params.get("e") === "1" && msg) {
    const { apiErrorMessage } = await import("../i18n/index");
    msg.textContent = apiErrorMessage("unauthorized");
  }
  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (meRes.ok) {
    location.replace(`/account-clubs.html?next=${encodeURIComponent(nextAfterLogin)}`);
  }
})();