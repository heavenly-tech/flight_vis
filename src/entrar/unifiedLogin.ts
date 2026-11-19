import { applyI18nDocument, bootI18n, t, getLocale } from "../i18n/boot";
import { apiErrorMessage } from "../i18n/index";
import { safeNextPath } from "../club/safeNext";

export async function bootUnifiedLogin(): Promise<void> {
  const form = document.getElementById("login-form") as HTMLFormElement;
  const msg = document.getElementById("msg") as HTMLParagraphElement;
  form.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

  await bootI18n({ switcher: "#lang-slot", persistUser: false });
  applyI18nDocument(document);

  const params = new URLSearchParams(location.search);
  const next = safeNextPath(params.get("next") ?? "/");

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (meRes.ok) {
    location.replace(`/account-clubs.html?next=${encodeURIComponent(next)}`);
    return;
  }

  const nextEl = document.getElementById("next") as HTMLInputElement | null;
  nextEl?.setAttribute("value", next);

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(form));
    const email = String(body.email ?? "").trim();
    const password = String(body.password ?? "");
    if (!email) return;
    if (!password) {
      msg.textContent = t("club.login.passwordOrMagic");
      return;
    }
    const res = await fetch("/api/account/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ email, password }),
    });
    if (!res.ok) {
      const err = (await res.json().catch(() => ({}))).error ?? "unauthorized";
      msg.textContent = apiErrorMessage(String(err));
      return;
    }
    location.replace(`/account-clubs.html?next=${encodeURIComponent(next)}`);
  });

  document.getElementById("magic-btn")?.addEventListener("click", async () => {
    const email = (form.querySelector('input[name="email"]') as HTMLInputElement).value.trim();
    if (!email) return;
    const res = await fetch("/api/account/magic-link", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ email, locale: getLocale() }),
    });
    msg.textContent = res.ok ? t("club.login.magicSent") : t("club.login.failed");
  });
}