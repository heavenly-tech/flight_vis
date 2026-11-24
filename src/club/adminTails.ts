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

interface AircraftRow {
  id: string;
  tail: string;
  type_code: string;
  active: boolean;
}

export interface AdminTailsController {
  refresh: () => Promise<void>;
}

export function mountAdminTails(): AdminTailsController {
  const list = document.querySelector<HTMLUListElement>("#list")!;
  const form = document.querySelector<HTMLFormElement>("#form")!;
  const statusEl = document.querySelector<HTMLParagraphElement>("#tails-status")!;

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
    const data = await api<{ aircraft: AircraftRow[] }>("/api/admin/aircraft");
    if (data.aircraft.length === 0) {
      const empty = document.createElement("li");
      empty.className = "muted";
      empty.textContent = t("admin.tails.empty");
      list.replaceChildren(empty);
      return;
    }
    list.replaceChildren(
      ...data.aircraft.map((a) => {
        const li = document.createElement("li");
        const statusText = a.active
          ? t("admin.tails.statusActive")
          : t("admin.tails.statusInactive");
        const btn = document.createElement("button");
        btn.type = "button";
        btn.textContent = a.active ? t("admin.deactivate") : t("admin.activate");
        btn.addEventListener("click", async () => {
          clearStatus();
          try {
            const res = await fetch("/api/admin/aircraft", {
              method: "POST",
              credentials: "same-origin",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ tail: a.tail, type: a.type_code, active: !a.active }),
            });
            if (!res.ok) {
              const raw = await extractApiError(res);
              showError(raw ? apiErrorMessage(raw) : t("admin.tails.saveFailed"));
              return;
            }
            await refresh();
          } catch {
            showError(t("admin.tails.saveFailed"));
          }
        });
        li.append(`${a.tail} · ${a.type_code} · ${statusText} `, btn);
        return li;
      }),
    );
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    clearStatus();
    const fd = new FormData(form);
    try {
      const res = await fetch("/api/admin/aircraft", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          tail: String(fd.get("tail")).trim(),
          type: String(fd.get("type")).trim(),
          active: true,
        }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        showError(raw ? apiErrorMessage(raw) : t("admin.tails.saveFailed"));
        return;
      }
      form.reset();
      await refresh();
    } catch {
      showError(t("admin.tails.saveFailed"));
    }
  });

  return { refresh };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="admin-tails"]')) {
  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    const controller = mountAdminTails();
    try {
      await requireAdmin();
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
