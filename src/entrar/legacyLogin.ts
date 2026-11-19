import { applyI18nDocument, bootI18n, t } from "../i18n/boot";
import { safeNextPath } from "../club/safeNext";

export async function bootLegacyLogin(): Promise<void> {
  const legacyForm = document.getElementById("legacy-form") as HTMLFormElement | null;
  legacyForm?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });
  await bootI18n({ persistUser: false });
  applyI18nDocument(document);

  const params = new URLSearchParams(location.search);
  const nextEl = document.getElementById("next") as HTMLInputElement;
  nextEl.value = safeNextPath(params.get("next") ?? "/");
  const err = document.getElementById("err") as HTMLParagraphElement;
  if (params.get("e") === "1") err.textContent = t("legacyLogin.wrongPassword");

  const gate = await fetch("/api/auth/legacy-gate", { credentials: "same-origin" }).then((r) =>
    r.json(),
  ) as { allowed?: boolean; configured?: boolean; authenticated?: boolean };

  const denied = document.getElementById("gate-denied") as HTMLElement;
  const form = legacyForm!;
  const locked = document.getElementById("locked") as HTMLElement;

  if (!gate.allowed) {
    denied.hidden = false;
    return;
  }
  if (!gate.configured) {
    locked.hidden = false;
    return;
  }
  form.hidden = false;
  if (gate.authenticated) {
    location.replace(nextEl.value);
    return;
  }
  form.addEventListener("submit", () => {
    const btn = form.querySelector("button");
    if (btn) btn.setAttribute("disabled", "true");
  });
}