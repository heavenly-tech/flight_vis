import { apiErrorMessage, t } from "../i18n/index";
import { ensureClubAdminOrRedirect } from "./adminGate";
import {
  api,
  closeDialog,
  extractApiError,
  isClubAdminRedirect,
  mountAdminNav,
  openDialog,
  pendingAccessRequestsCount,
  requireAdmin,
  setNavRequestsBadge,
  type PendingCounts,
} from "./adminCommon";
import { startClubPage } from "./i18nPage";

interface AccessRequestItem {
  id: string;
  pilotName: string;
  pilotEmail: string;
  requestedTail: string;
  typeCode: string | null;
  newTypeName: string | null;
  status: string;
}

export interface AdminRequestsController {
  refresh: () => Promise<void>;
}

export function mountAdminRequests(): AdminRequestsController {
  const list = document.querySelector<HTMLDivElement>("#list")!;
  const badge = document.querySelector<HTMLSpanElement>("#badge")!;
  const statusEl = document.querySelector<HTMLParagraphElement>("#requests-status")!;
  const rejectDialog = document.querySelector<HTMLDialogElement>("#reject-dialog")!;
  const rejectForm = document.querySelector<HTMLFormElement>("#reject-form")!;
  const rejectSummary = document.querySelector<HTMLParagraphElement>("#reject-dialog-summary")!;
  const rejectReason = document.querySelector<HTMLTextAreaElement>("#reject-reason")!;
  const rejectCancel = document.querySelector<HTMLButtonElement>("#reject-cancel")!;

  let pendingReject: AccessRequestItem | null = null;

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

  function openRejectDialog(req: AccessRequestItem): void {
    pendingReject = req;
    rejectSummary.textContent = t("admin.requests.rejectSummary", {
      pilot: req.pilotName,
      tail: req.requestedTail,
    });
    rejectReason.value = "";
    openDialog(rejectDialog);
    rejectReason.focus();
  }

  rejectCancel.addEventListener("click", () => {
    pendingReject = null;
    closeDialog(rejectDialog);
  });

  rejectDialog.addEventListener("cancel", () => {
    pendingReject = null;
  });

  rejectForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    const target = pendingReject;
    pendingReject = null;
    const reason = rejectReason.value.trim() || t("admin.rejectDefault");
    closeDialog(rejectDialog);
    if (!target) return;
    clearStatus();
    try {
      const res = await fetch(`/api/admin/access-requests/${encodeURIComponent(target.id)}/reject`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ reason }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        showError(raw ? apiErrorMessage(raw) : t("admin.requests.actionFailed"));
        return;
      }
      await refresh();
    } catch {
      showError(t("admin.requests.actionFailed"));
    }
  });

  async function refresh(): Promise<void> {
    const [counts, pending] = await Promise.all([
      api<PendingCounts>("/api/admin/pending-counts"),
      api<{ requests: AccessRequestItem[] }>("/api/admin/access-requests?status=pending"),
    ]);
    const count = pendingAccessRequestsCount(counts);
    badge.hidden = count === 0;
    badge.textContent = String(count);
    setNavRequestsBadge(count);

    if (pending.requests.length === 0) {
      const empty = document.createElement("p");
      empty.className = "muted";
      empty.textContent = t("admin.requests.empty");
      list.replaceChildren(empty);
      return;
    }

    list.replaceChildren(
      ...pending.requests.map((r) => {
        const art = document.createElement("article");
        art.dataset.requestId = r.id;
        const type = r.typeCode ?? r.newTypeName ?? "?";
        const p = document.createElement("p");
        const strong = document.createElement("strong");
        strong.textContent = r.pilotName;
        p.append(strong, document.createTextNode(` (${r.pilotEmail})`));
        p.appendChild(document.createElement("br"));
        p.append(document.createTextNode(t("admin.requests.detail", { tail: r.requestedTail, type })));
        art.appendChild(p);

        const approve = document.createElement("button");
        approve.type = "button";
        approve.className = "btn-approve";
        approve.textContent = t("admin.approve");
        approve.addEventListener("click", async () => {
          clearStatus();
          try {
            const res = await fetch(
              `/api/admin/access-requests/${encodeURIComponent(r.id)}/approve`,
              {
                method: "POST",
                credentials: "same-origin",
                headers: { "Content-Type": "application/json" },
                body: "{}",
              },
            );
            if (!res.ok) {
              const raw = await extractApiError(res);
              showError(raw ? apiErrorMessage(raw) : t("admin.requests.actionFailed"));
              return;
            }
            await refresh();
          } catch {
            showError(t("admin.requests.actionFailed"));
          }
        });

        const reject = document.createElement("button");
        reject.type = "button";
        reject.className = "btn-reject";
        reject.textContent = t("admin.reject");
        reject.addEventListener("click", () => openRejectDialog(r));

        art.append(approve, reject);
        return art;
      }),
    );
  }

  return { refresh };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="admin-requests"]')) {
  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    const controller = mountAdminRequests();
    try {
      await requireAdmin();
      await controller.refresh();
    } catch (e) {
      if (!isClubAdminRedirect(e)) {
        const list = document.querySelector<HTMLDivElement>("#list");
        if (list) list.textContent = t("admin.adminRequired");
      }
    }
  })();
}
