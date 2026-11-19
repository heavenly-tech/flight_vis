import { applyI18nDocument, bootI18n, t } from "./i18n/boot";
import { safeNextPath } from "./club/safeNext";

const q = new URLSearchParams(location.search);
const err = document.getElementById("err");
const next = document.getElementById("next") as HTMLInputElement | null;
const dest = safeNextPath(q.get("next") ?? "/");
if (next) next.value = dest;

function showLocked(): void {
  const body = document.getElementById("body");
  if (!body) return;
  body.innerHTML =
    '<p class="lock">Upload is locked. Administrator must set <code>FLIGHT_VIS_PASSWORD</code>.</p>';
}

if (q.get("locked") === "1") showLocked();

const loginForm = document.getElementById("login-form");
loginForm?.addEventListener("submit", () => {
  const button = loginForm.querySelector("button");
  if (button) button.disabled = true;
});

void fetch("/api/auth/me", { credentials: "same-origin" })
  .then((res) =>
    res.ok ? res.json() : { authenticated: false, configured: true },
  )
  .then((me: { authenticated?: boolean; configured?: boolean }) => {
    if (me.authenticated) location.replace(dest);
    else if (!me.configured) showLocked();
  })
  .catch(() => {});

void bootI18n().then(() => {
  applyI18nDocument(document);
  if (q.get("e") === "1" && err) err.textContent = t("auth.wrongPassword");
  const clubBtn = document.getElementById("club-login-btn");
  if (!clubBtn) return;
  clubBtn.textContent = t("nav.clubLogin");
  const rawNext = new URLSearchParams(location.search).get("next");
  if (rawNext) {
    const safe = safeNextPath(rawNext);
    clubBtn.setAttribute(
      "href",
      `/club-login.html?${new URLSearchParams({ next: safe })}`,
    );
  }
});