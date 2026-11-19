import { startClubPage } from "./i18nPage";
import { apiErrorMessage, t } from "../i18n/index";

await startClubPage({ persistUser: false });

const form = document.getElementById("form") as HTMLFormElement;
const msg = document.getElementById("msg") as HTMLParagraphElement;

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = Object.fromEntries(new FormData(form));
  const payload: Record<string, string> = { email: String(body.email ?? "") };
  const pw = String(body.password ?? "");
  if (pw) payload.password = pw;
  const res = await fetch("/api/account/signup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(payload),
  });
  if (res.ok) {
    msg.textContent = t("account.signup.success");
    if (pw) location.href = "/login";
  } else {
    const err = (await res.json().catch(() => ({}))).error ?? "signup failed";
    msg.textContent = apiErrorMessage(String(err));
  }
});