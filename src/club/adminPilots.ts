import { apiErrorMessage, t } from "../i18n/index";
import { ensureClubAdminOrRedirect } from "./adminGate";
import {
  api,
  closeDialog,
  extractApiError,
  isClubAdminRedirect,
  mountAdminNav,
  openDialog,
  requireAdmin,
  setNavPilotsBadge,
} from "./adminCommon";
import { startClubPage } from "./i18nPage";

interface PendingPilotItem {
  id: string;
  name: string;
  email: string;
}

interface MembershipItem {
  id: string;
  name: string;
  email: string;
  status: string;
  role: string;
}

export interface AdminPilotsController {
  refresh: () => Promise<void>;
}

export function roleLabel(role: string): string {
  if (role === "admin") return t("club.admin.roleAdmin");
  if (role === "staff") return t("club.admin.roleStaff");
  return t("club.admin.rolePilot");
}

export function membershipStatusLabel(status: string): string {
  const norm = status.toLowerCase();
  if (norm === "active" || norm === "approved") return t("club.admin.statusActive");
  if (norm === "pending") return t("club.admin.statusPending");
  if (norm === "suspended") return t("club.admin.statusSuspended");
  return status;
}

export function mountAdminPilots(): AdminPilotsController {
  const pending = document.querySelector<HTMLUListElement>("#pending")!;
  const memberships = document.querySelector<HTMLDivElement>("#memberships")!;
  const membershipsStatus = document.querySelector<HTMLParagraphElement>("#memberships-status")!;
  const inviteForm = document.querySelector<HTMLFormElement>("#invite-form")!;
  const inviteEmail = document.querySelector<HTMLInputElement>("#invite-email")!;
  const inviteStatus = document.querySelector<HTMLParagraphElement>("#invite-status")!;
  const removeDialog = document.querySelector<HTMLDialogElement>("#remove-dialog")!;
  const removeBody = document.querySelector<HTMLParagraphElement>("#remove-dialog-body")!;
  const removeCancel = document.querySelector<HTMLButtonElement>("#remove-cancel")!;
  const removeConfirm = document.querySelector<HTMLButtonElement>("#remove-confirm")!;

  let pendingRemove: MembershipItem | null = null;

  function clearMembershipStatus(): void {
    membershipsStatus.hidden = true;
    membershipsStatus.textContent = "";
    membershipsStatus.classList.remove("status-error", "status-ok");
    membershipsStatus.removeAttribute("role");
    membershipsStatus.setAttribute("aria-live", "polite");
  }

  function showMembershipError(message: string): void {
    membershipsStatus.hidden = false;
    membershipsStatus.textContent = message;
    membershipsStatus.classList.remove("status-ok");
    membershipsStatus.classList.add("status-error");
    membershipsStatus.setAttribute("role", "alert");
    membershipsStatus.setAttribute("aria-live", "assertive");
  }

  function setInviteSuccess(message: string): void {
    inviteStatus.setAttribute("role", "status");
    inviteStatus.setAttribute("aria-live", "polite");
    inviteStatus.classList.remove("status-error");
    inviteStatus.classList.add("status-ok");
    inviteStatus.textContent = message;
  }

  function setInviteError(message: string): void {
    inviteStatus.setAttribute("role", "alert");
    inviteStatus.setAttribute("aria-live", "assertive");
    inviteStatus.classList.remove("status-ok");
    inviteStatus.classList.add("status-error");
    inviteStatus.textContent = message;
  }

  async function postAction(path: string, jsonBody?: string): Promise<boolean> {
    clearMembershipStatus();
    try {
      const init: RequestInit = { method: "POST", credentials: "same-origin" };
      if (jsonBody !== undefined) {
        init.headers = { "Content-Type": "application/json" };
        init.body = jsonBody;
      }
      const res = await fetch(path, init);
      if (!res.ok) {
        const raw = await extractApiError(res);
        if (raw === "last_admin") {
          showMembershipError(t("club.admin.lastAdmin"));
          return false;
        }
        showMembershipError(raw ? apiErrorMessage(raw) : t("club.admin.actionFailed"));
        return false;
      }
      return true;
    } catch {
      showMembershipError(t("club.admin.actionFailed"));
      return false;
    }
  }

  function openRemoveDialog(m: MembershipItem): void {
    pendingRemove = m;
    removeBody.textContent = t("club.admin.removeConfirmBody", {
      name: m.name,
      email: m.email,
    });
    openDialog(removeDialog);
  }

  removeCancel.addEventListener("click", () => {
    pendingRemove = null;
    closeDialog(removeDialog);
  });

  removeDialog.addEventListener("cancel", () => {
    pendingRemove = null;
  });

  removeConfirm.addEventListener("click", async () => {
    const target = pendingRemove;
    pendingRemove = null;
    closeDialog(removeDialog);
    if (!target) return;
    const ok = await postAction(`/api/admin/memberships/${encodeURIComponent(target.id)}/remove`);
    if (ok) await refresh();
  });

  async function refresh(): Promise<void> {
    const pend = await api<{ pilots: PendingPilotItem[] }>("/api/admin/pilots/pending");
    setNavPilotsBadge(pend.pilots.length);
    if (pend.pilots.length === 0) {
      const empty = document.createElement("li");
      empty.className = "muted";
      empty.textContent = t("club.admin.noPendingPilots");
      pending.replaceChildren(empty);
    } else {
      pending.replaceChildren(
        ...pend.pilots.map((p) => {
          const li = document.createElement("li");
          const btn = document.createElement("button");
          btn.type = "button";
          btn.textContent = t("admin.approve");
          btn.addEventListener("click", async () => {
            const ok = await postAction(
              `/api/admin/pilots/${encodeURIComponent(p.id)}/approve`,
              "{}",
            );
            if (ok) await refresh();
          });
          li.append(`${p.name} (${p.email}) `, btn);
          return li;
        }),
      );
    }

    const roster = await api<{ memberships: MembershipItem[] }>("/api/admin/memberships");
    if (roster.memberships.length === 0) {
      const empty = document.createElement("p");
      empty.className = "muted";
      empty.textContent = t("club.admin.noMemberships");
      memberships.replaceChildren(empty);
      return;
    }

    memberships.replaceChildren(
      ...roster.memberships.map((m) => {
        const row = document.createElement("div");
        row.className = "mem-row";
        row.dataset.membershipId = m.id;
        const title = document.createElement("div");
        const badge = document.createElement("span");
        badge.className = "badge";
        badge.textContent = `${roleLabel(m.role)} · ${membershipStatusLabel(m.status)}`;
        title.append(`${m.name} · ${m.email} `, badge);
        row.appendChild(title);

        const actions = document.createElement("div");
        actions.className = "mem-actions";

        if (m.role !== "admin" && m.status === "active") {
          const promote = document.createElement("button");
          promote.type = "button";
          promote.textContent = t("club.admin.promote");
          promote.addEventListener("click", async () => {
            const ok = await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/promote`);
            if (ok) await refresh();
          });
          actions.appendChild(promote);
        }
        if (m.role === "admin" && m.status === "active") {
          const demote = document.createElement("button");
          demote.type = "button";
          demote.textContent = t("club.admin.demote");
          demote.addEventListener("click", async () => {
            const ok = await postAction(`/api/admin/memberships/${encodeURIComponent(m.id)}/demote`);
            if (ok) await refresh();
          });
          const remove = document.createElement("button");
          remove.type = "button";
          remove.className = "btn-remove-access";
          remove.textContent = t("club.admin.remove");
          remove.addEventListener("click", () => openRemoveDialog(m));
          actions.appendChild(demote);
          actions.appendChild(remove);
        } else if (m.status === "active") {
          const remove = document.createElement("button");
          remove.type = "button";
          remove.className = "btn-remove-access";
          remove.textContent = t("club.admin.remove");
          remove.addEventListener("click", () => openRemoveDialog(m));
          actions.appendChild(remove);
        }

        if (actions.childElementCount > 0) {
          row.appendChild(actions);
        }
        return row;
      }),
    );
  }

  inviteForm.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

  inviteForm.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    inviteStatus.textContent = "";
    inviteStatus.classList.remove("status-error", "status-ok");
    const email = inviteEmail.value.trim();
    if (!email) return;
    try {
      const res = await fetch("/api/admin/memberships/invite", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        setInviteError(raw ? apiErrorMessage(raw) : t("club.admin.inviteFailed"));
        return;
      }
      setInviteSuccess(t("club.admin.inviteSent"));
      inviteEmail.value = "";
    } catch {
      setInviteError(t("club.admin.inviteFailed"));
    }
  });

  return { refresh };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="admin-pilots"]')) {
  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    const controller = mountAdminPilots();
    try {
      await requireAdmin();
      await controller.refresh();
    } catch (e) {
      if (!isClubAdminRedirect(e)) {
        const pending = document.querySelector<HTMLUListElement>("#pending");
        if (pending) {
          const li = document.createElement("li");
          li.textContent = t("api.adminOnly");
          pending.replaceChildren(li);
        }
      }
    }
  })();
}
