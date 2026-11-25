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
} from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { renderTextWithLink } from "./inlineLink";

interface PendingPilotItem {
  id: string;
  name: string;
  email: string;
}

interface MembershipItem {
  id: string;
  pilotId?: string;
  name: string;
  email: string;
  status: string;
  role: string;
}

interface PilotCheckoutItem {
  id: string;
  pilot_id: string;
  type_code: string;
  tail: string | null;
  expires_at: number | null;
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
  const pendingNotice = document.querySelector<HTMLParagraphElement>("#pending-pilots-notice");
  const memberships = document.querySelector<HTMLDivElement>("#memberships")!;
  const membershipsStatus = document.querySelector<HTMLParagraphElement>("#memberships-status")!;
  const inviteForm = document.querySelector<HTMLFormElement>("#invite-form")!;
  const inviteEmail = document.querySelector<HTMLInputElement>("#invite-email")!;
  const inviteRole = document.querySelector<HTMLSelectElement>("#invite-role");
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
    const [roster, pendingPilots, checkoutsResult] = await Promise.all([
      api<{ memberships: MembershipItem[] }>("/api/admin/memberships"),
      api<{ pilots: PendingPilotItem[] }>("/api/admin/pilots/pending")
        .then((d) => d.pilots)
        .catch(() => null),
      api<{ checkouts: PilotCheckoutItem[] }>("/api/admin/checkouts")
        .then((d) => ({ ok: true as const, checkouts: d.checkouts }))
        .catch(() => ({ ok: false as const, checkouts: [] as PilotCheckoutItem[] })),
    ]);

    const visibleMemberships = roster.memberships.filter(
      (m) => m.status.toLowerCase() !== "pending",
    );
    const pendingFromMemberships = roster.memberships.length - visibleMemberships.length;
    const pendingCount = Math.max(pendingPilots?.length ?? 0, pendingFromMemberships);

    if (pendingNotice) {
      if (pendingCount > 0) {
        const template =
          pendingCount === 1
            ? t("club.admin.pendingNoticeOne")
            : t("club.admin.pendingNoticeMany", { count: pendingCount });
        renderTextWithLink(pendingNotice, template, "/club-admin-requests.html", t("nav.requests"));
        pendingNotice.hidden = false;
      } else {
        pendingNotice.hidden = true;
        pendingNotice.replaceChildren();
      }
    }

    const now = Date.now();
    const ratingsByPilot = new Map<string, PilotCheckoutItem[]>();
    if (checkoutsResult.ok) {
      for (const c of checkoutsResult.checkouts) {
        if (c.expires_at != null && c.expires_at <= now) continue;
        const bucket = ratingsByPilot.get(c.pilot_id) ?? [];
        bucket.push(c);
        ratingsByPilot.set(c.pilot_id, bucket);
      }
    }

    if (visibleMemberships.length === 0) {
      const empty = document.createElement("p");
      empty.className = "muted";
      empty.textContent = t("club.admin.noMemberships");
      memberships.replaceChildren(empty);
      return;
    }

    memberships.replaceChildren(
      ...visibleMemberships.map((m) => {
        const row = document.createElement("div");
        row.className = "admin-row mem-row";
        row.dataset.membershipId = m.id;

        const identity = document.createElement("div");
        identity.className = "mem-identity";
        const badge = document.createElement("span");
        badge.className = "badge";
        badge.textContent = `${roleLabel(m.role)} · ${membershipStatusLabel(m.status)}`;
        identity.append(`${m.name} · ${m.email} `, badge);
        row.appendChild(identity);

        const ratings = document.createElement("div");
        ratings.className = "mem-ratings";
        if (checkoutsResult.ok) {
          const activeRatings = m.pilotId ? (ratingsByPilot.get(m.pilotId) ?? []) : [];
          if (activeRatings.length > 0) {
            for (const c of activeRatings) {
              const chip = document.createElement("span");
              chip.className = "rating-chip";
              const tailText = c.tail ?? t("admin.checkouts.allTails");
              chip.textContent = `${c.type_code} · ${tailText}`;
              ratings.appendChild(chip);
            }
          } else {
            const noRatings = document.createElement("span");
            noRatings.className = "muted";
            noRatings.textContent = t("club.admin.noRatings");
            ratings.appendChild(noRatings);
          }
        }
        const enableLink = document.createElement("a");
        enableLink.className = "rating-link";
        enableLink.href = m.pilotId
          ? `/club-admin-checkouts.html?pilot=${encodeURIComponent(m.pilotId)}`
          : "/club-admin-checkouts.html";
        enableLink.textContent = t("club.admin.enablePilot");
        ratings.appendChild(enableLink);
        row.appendChild(ratings);

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
    const role: "pilot" | "admin" = inviteRole?.value === "admin" ? "admin" : "pilot";
    try {
      const res = await fetch("/api/admin/memberships/invite", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email, role }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        setInviteError(raw ? apiErrorMessage(raw) : t("club.admin.inviteFailed"));
        return;
      }
      setInviteSuccess(
        role === "admin"
          ? t("club.admin.inviteSentAdmin")
          : t("club.admin.inviteSentPilot"),
      );
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
        const memberships = document.querySelector<HTMLDivElement>("#memberships");
        if (memberships) {
          const p = document.createElement("p");
          p.textContent = t("api.adminOnly");
          memberships.replaceChildren(p);
        }
      }
    }
  })();
}
