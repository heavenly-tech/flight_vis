import { startClubPage } from "./i18nPage";
import { t, apiErrorMessage } from "../i18n/boot";

void (async () => {
  await startClubPage({ persistUser: false });
  const form = document.getElementById("form") as HTMLFormElement | null;
  const msg = document.getElementById("msg");
  if (!form || !msg) return;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(form));
    const res = await fetch("/api/club/register", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    msg.textContent = res.ok
      ? t("club.register.success")
      : apiErrorMessage((await res.json().catch(() => ({}))).error ?? "register failed");
  });
})();