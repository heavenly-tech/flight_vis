import { apiErrorMessage, t } from "../i18n/index";

const form = document.getElementById("form") as HTMLFormElement | null;
const msg = document.getElementById("msg") as HTMLParagraphElement | null;

form?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

declare global {
  interface Window {
    __fvAccountSignupReady?: boolean;
  }
}
window.__fvAccountSignupReady = true;

form?.addEventListener("submit", async () => {
  if (!form) return;
  const body = Object.fromEntries(new FormData(form));
  const payload: Record<string, string> = { email: String(body.email ?? "") };
  const pw = String(body.password ?? "");
  if (pw) payload.password = pw;
  const res = await fetch("/api/account/signup", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(payload),
  });
  if (res.ok) {
    if (msg) msg.textContent = t("account.signup.success");
    if (pw) location.href = "/login";
  } else {
    const err = (await res.json().catch(() => ({}))).error ?? "signup failed";
    if (msg) msg.textContent = apiErrorMessage(String(err));
  }
});

void (async () => {
  const { startClubPage } = await import("./i18nPage");
  await startClubPage({ persistUser: false });
})();