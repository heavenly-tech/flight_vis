import { safeNextPath } from "./safeNext";

const nextAfterLogin = safeNextPath(
  new URLSearchParams(location.search).get("next") ?? "/club-pilot.html",
);

const form = document.querySelector<HTMLFormElement>("#form");
const msg = document.querySelector<HTMLParagraphElement>("#msg");
const nextInput = document.querySelector<HTMLInputElement>("#next");

if (nextInput) nextInput.value = nextAfterLogin;

if (form && msg) {
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    msg.textContent = "";
    const body = Object.fromEntries(new FormData(form));
    const res = await fetch("/api/club/login", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json",
      },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    if (res.ok) {
      location.replace(nextAfterLogin);
      return;
    }
    const err =
      ((await res.json().catch(() => ({}))) as { error?: string }).error ??
      "unauthorized";
    const { apiErrorMessage } = await import("../i18n/boot");
    msg.textContent = apiErrorMessage(err);
  });
}

declare global {
  interface Window {
    __fvClubLoginReady?: boolean;
  }
}

window.__fvClubLoginReady = true;

void (async () => {
  const { startClubPage } = await import("./i18nPage");
  const { apiErrorMessage } = await import("../i18n/boot");
  await startClubPage();
  const q = new URLSearchParams(location.search);
  if (q.get("e") === "1" && msg) {
    msg.textContent = apiErrorMessage("unauthorized");
  }
  const existing = await fetch("/api/club/me", { credentials: "same-origin" });
  if (existing.ok) {
    const me = (await existing.json()) as { pilot?: unknown };
    if (me.pilot) location.replace(nextAfterLogin);
  }
})();