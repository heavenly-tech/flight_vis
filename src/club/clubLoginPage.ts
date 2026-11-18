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
    const res = await fetch("/api/club/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    if (res.ok) {
      msg.textContent = t("club.login.success");
      location.href = "/club-pilot.html";
    } else {
      const err = (await res.json().catch(() => ({}))).error ?? "unauthorized";
      msg.textContent = apiErrorMessage(String(err));
    }
  });
})();