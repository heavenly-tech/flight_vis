import { startClubPage } from "../club/i18nPage";
import { applyI18nDocument, apiErrorMessage, t } from "../i18n/index";

type AccountMe = {
  skipCurrentPassword?: boolean;
  account?: { email?: string };
};

export async function bootAccountPage(): Promise<void> {
  await startClubPage({ persistUser: true });
  applyI18nDocument(document);

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (!meRes.ok) {
    location.replace(`/login?next=${encodeURIComponent("/account.html")}`);
    return;
  }
  const me = (await meRes.json()) as AccountMe;
  const email = me.account?.email ?? "";
  const emailLine = document.getElementById("email-line");
  if (emailLine) emailLine.textContent = email;

  const skip = me.skipCurrentPassword === true;
  const currentWrap = document.getElementById("current-wrap");
  const currentInput = document.querySelector<HTMLInputElement>("#current");
  const currentLabel = document.getElementById("current-label");
  if (currentWrap && currentInput && currentLabel) {
    if (skip) {
      currentLabel.dataset.i18n = "account.password.currentOptionalMagic";
      applyI18nDocument(currentWrap);
      currentInput.removeAttribute("required");
    } else {
      currentLabel.dataset.i18n = "account.password.current";
      applyI18nDocument(currentWrap);
      currentInput.setAttribute("required", "");
    }
  }

  const form = document.getElementById("password-form") as HTMLFormElement | null;
  const msg = document.getElementById("msg");
  form?.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (msg) msg.textContent = "";
    const body = Object.fromEntries(new FormData(form));
    const payload: { password: string; current_password?: string } = {
      password: String(body.password ?? ""),
    };
    const current = String(body.current_password ?? "").trim();
    if (current) payload.current_password = current;
    const res = await fetch("/api/account/password", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(payload),
    });
    if (res.ok) {
      if (msg) msg.textContent = t("club.login.success");
      return;
    }
    const err = ((await res.json().catch(() => ({}))) as { error?: string }).error ?? "unauthorized";
    if (msg) msg.textContent = apiErrorMessage(String(err));
  });
}

void bootAccountPage();