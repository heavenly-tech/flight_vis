import { t } from "../i18n/index";

const form = document.getElementById("form") as HTMLFormElement | null;
const msg = document.getElementById("msg");

form?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

declare global {
  interface Window {
    __fvClubSignupReady?: boolean;
  }
}
window.__fvClubSignupReady = true;

const slugInput = form?.elements.namedItem("slug") as HTMLInputElement | undefined;
slugInput?.addEventListener("input", () => {
  slugInput.value = slugInput.value.toLowerCase().replace(/[^a-z0-9-]/g, "");
});

form?.addEventListener("submit", async () => {
  if (!form || !msg) return;
  const body = Object.fromEntries(new FormData(form).entries());
  const res = await fetch("/api/clubs/signup", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(body),
  });
  msg.textContent = res.ok ? t("club.signup.success") : t("club.signup.failed");
  if (res.ok) form.reset();
});

void (async () => {
  const { startClubPage } = await import("./i18nPage");
  await startClubPage({ persistUser: false });
})();