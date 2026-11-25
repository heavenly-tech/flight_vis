import { apiErrorMessage, t } from "../i18n/index";
import { ensureClubAdminOrRedirect } from "./adminGate";
import {
  api,
  extractApiError,
  isClubAdminRedirect,
  mountAdminNav,
  requireAdmin,
} from "./adminCommon";
import { startClubPage } from "./i18nPage";

export interface AdminTypesController {
  refresh: () => Promise<void>;
}

export function mountAdminTypes(): AdminTypesController {
  const list = document.querySelector<HTMLUListElement>("#list")!;
  const code = document.querySelector<HTMLInputElement>("#code")!;
  const form = document.querySelector<HTMLFormElement>("#form")!;
  const statusEl = document.querySelector<HTMLParagraphElement>("#types-status")!;

  function clearStatus(): void {
    statusEl.hidden = true;
    statusEl.textContent = "";
    statusEl.classList.remove("status-error", "status-ok");
    statusEl.removeAttribute("role");
    statusEl.setAttribute("aria-live", "polite");
  }

  function showError(message: string): void {
    statusEl.hidden = false;
    statusEl.textContent = message;
    statusEl.classList.remove("status-ok");
    statusEl.classList.add("status-error");
    statusEl.setAttribute("role", "alert");
    statusEl.setAttribute("aria-live", "assertive");
  }

  async function refresh(): Promise<void> {
    const data = await api<{ types: { id: string; code: string }[] }>("/api/admin/aircraft-types");
    if (data.types.length === 0) {
      const empty = document.createElement("li");
      empty.className = "muted";
      empty.textContent = t("admin.types.empty");
      list.replaceChildren(empty);
      return;
    }
    list.replaceChildren(
      ...data.types.map((item) => {
        const li = document.createElement("li");
        li.textContent = item.code;
        return li;
      }),
    );
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    clearStatus();
    try {
      const res = await fetch("/api/admin/aircraft-types", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: code.value.trim() }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        showError(raw ? apiErrorMessage(raw) : t("admin.types.saveFailed"));
        return;
      }
      code.value = "";
      await refresh();
    } catch {
      showError(t("admin.types.saveFailed"));
    }
  });

  return { refresh };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="admin-types"]')) {
  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    const controller = mountAdminTypes();
    try {
      const who = document.querySelector<HTMLParagraphElement>("#who");
      const admin = await requireAdmin();
      if (who) who.textContent = t("admin.types.signedInAs", { name: admin.name });
      await controller.refresh();
    } catch (e) {
      if (!isClubAdminRedirect(e)) {
        const list = document.querySelector<HTMLUListElement>("#list");
        if (list) {
          const li = document.createElement("li");
          li.textContent = t("admin.adminRequired");
          list.replaceChildren(li);
        }
      }
    }
  })();
}
