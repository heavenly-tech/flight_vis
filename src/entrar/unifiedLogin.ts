import { safeNextPath } from "../club/safeNext";

const params = new URLSearchParams(location.search);
let nextAfterLogin = safeNextPath(params.get("next") ?? "/");
const passwordMode = params.get("mode") === "password";

const magicPanel = document.getElementById("magic-panel");
const passwordPanel = document.getElementById("password-panel");
const nextInput = document.querySelector<HTMLInputElement>("#next");
const legacyNext = document.querySelector<HTMLInputElement>("#legacy-next");
const magicMsg = document.querySelector<HTMLParagraphElement>("#magic-msg");
const passwordMsg = document.querySelector<HTMLParagraphElement>("#password-msg");
const emailMagic = document.querySelector<HTMLInputElement>("#email-magic");
const passwordForm = document.querySelector<HTMLFormElement>("#password-form");
const usePassword = document.getElementById("use-password");
const backMagic = document.getElementById("back-magic");

function syncNextInputs(): void {
  if (nextInput) nextInput.value = nextAfterLogin;
  if (legacyNext) legacyNext.value = nextAfterLogin;
}

function applyModeUi(): void {
  if (passwordMode) {
    magicPanel?.setAttribute("hidden", "");
    passwordPanel?.removeAttribute("hidden");
  } else {
    passwordPanel?.setAttribute("hidden", "");
    magicPanel?.removeAttribute("hidden");
  }
}

function linkWithNext(href: string): string {
  const q = new URLSearchParams();
  if (href.includes("mode=password")) q.set("mode", "password");
  if (nextAfterLogin !== "/") q.set("next", nextAfterLogin);
  const qs = q.toString();
  return qs ? `${href.split("?")[0]}?${qs}` : href.split("?")[0];
}

declare global {
  interface Window {
    __fvUnifiedLoginReady?: boolean;
  }
}

window.__fvUnifiedLoginReady = true;

document.getElementById("magic-btn")?.removeAttribute("hidden");

applyModeUi();
syncNextInputs();

if (usePassword instanceof HTMLAnchorElement) {
  usePassword.href = linkWithNext("/login?mode=password");
}
if (backMagic instanceof HTMLAnchorElement) {
  backMagic.href = linkWithNext("/login");
}

passwordForm?.addEventListener("submit", () => {
  const button = passwordForm.querySelector("button[type=submit]");
  if (button instanceof HTMLButtonElement) button.disabled = true;
});

document.getElementById("magic-btn")?.addEventListener("click", async () => {
  const email = emailMagic?.value.trim() ?? "";
  if (!email) return;
  const { t, getLocale } = await import("../i18n/index");
  const res = await fetch("/api/account/magic-link", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ email, locale: getLocale() }),
  });
  if (magicMsg) {
    magicMsg.textContent = res.ok ? t("club.login.magicSent") : t("club.login.failed");
  }
});

passwordForm?.addEventListener("input", () => {
  const emailField = passwordForm.querySelector<HTMLInputElement>('input[name="email"]');
  const magicEmail = emailMagic?.value.trim();
  if (emailField && magicEmail && !emailField.value) {
    emailField.value = magicEmail;
  }
});

void (async () => {
  const { bootI18n, applyI18nDocument } = await import("../i18n/boot");
  await bootI18n({ switcher: "#lang-slot", persistUser: false });
  applyI18nDocument(document);
  nextAfterLogin = safeNextPath(
    new URLSearchParams(location.search).get("next") ?? "/",
  );
  syncNextInputs();
  if (usePassword instanceof HTMLAnchorElement) {
    usePassword.href = linkWithNext("/login?mode=password");
  }
  if (backMagic instanceof HTMLAnchorElement) {
    backMagic.href = linkWithNext("/login");
  }
  const err = params.get("e") === "1";
  if (err) {
    const { apiErrorMessage } = await import("../i18n/index");
    const text = apiErrorMessage("unauthorized");
    if (passwordMode && passwordMsg) passwordMsg.textContent = text;
    else if (magicMsg) magicMsg.textContent = text;
  }
  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (meRes.ok) {
    location.replace(`/account-clubs.html?next=${encodeURIComponent(nextAfterLogin)}`);
  }
})();