import { startClubPage } from "./i18nPage";
import { t } from "../i18n/index";

await startClubPage({ persistUser: false });

const form = document.getElementById("form") as HTMLFormElement;
const msg = document.getElementById("msg")!;

const slugInput = form.elements.namedItem("slug") as HTMLInputElement;
slugInput?.addEventListener("input", () => {
  slugInput.value = slugInput.value.toLowerCase().replace(/[^a-z0-9-]/g, "");
});

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const data = new FormData(form);
  const body = Object.fromEntries(data.entries());
  const res = await fetch("/api/clubs/signup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  msg.textContent = res.ok ? t("club.signup.success") : t("club.signup.failed");
  if (res.ok) form.reset();
});