import { performAccountLogout } from "../club/adminCommon";
import { startClubPage } from "../club/i18nPage";
import { safeNextPath } from "../club/safeNext";
import { apiErrorMessage, applyI18nDocument, t } from "../i18n/index";

export interface AccountMembership {
  id: string;
  clubId?: string;
  name?: string;
  slug?: string;
  status: string;
  role?: string;
}

export interface AccountMeResponse {
  account?: {
    id?: string;
    email?: string;
  };
  activeMembershipId?: string | null;
  memberships?: AccountMembership[];
}

export function membershipStatusLabel(status: string): string {
  if (status === "active") return t("club.login.statusActive");
  if (status === "pending") return t("club.login.statusPending");
  if (status === "rejected") return t("club.login.statusRejected");
  if (status === "revoked") return t("club.login.statusRevoked");
  if (status === "suspended") return t("club.login.statusSuspended");
  return status;
}

export function membershipRoleLabel(role: string | undefined): string {
  if (role === "admin") return t("club.admin.roleAdmin");
  if (role === "staff") return t("club.admin.roleStaff");
  return t("club.admin.rolePilot");
}

export function isStaffRole(role: string | undefined): boolean {
  return role === "admin" || role === "staff";
}

export function shouldShowContinue(search: string): boolean {
  const rawNext = new URLSearchParams(search).get("next");
  if (rawNext === null || rawNext.trim() === "") return false;
  return safeNextPath(rawNext) !== "/";
}

export function resolveAdminNext(search: string): string {
  const raw = new URLSearchParams(search).get("adminNext");
  return safeNextPath(raw ?? "/club-admin-map.html");
}

export function findSoleAutoActivateMembershipId(
  activeMembershipId: string | null | undefined,
  memberships: readonly AccountMembership[],
): string | null {
  if (activeMembershipId) {
    const activeMatch = memberships.find(
      (m) => m.id === activeMembershipId && m.status === "active",
    );
    if (activeMatch) return null;
  }
  const activeMemberships = memberships.filter((m) => m.status === "active");
  return activeMemberships.length === 1 ? activeMemberships[0].id : null;
}

export interface BootMisClubsOptions {
  navigate?: (url: string) => void;
  replace?: (url: string) => void;
}

export async function bootMisClubs(opts: BootMisClubsOptions = {}): Promise<void> {
  const navigate = opts.navigate ?? ((url: string) => {
    location.href = url;
  });
  const replace = opts.replace ?? ((url: string) => {
    location.replace(url);
  });

  const joinFormEl = document.getElementById("join-form") as HTMLFormElement | null;
  joinFormEl?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

  await startClubPage({ persistUser: true });
  applyI18nDocument(document);

  const params = new URLSearchParams(location.search);
  const nextAfterLogin = safeNextPath(params.get("next") ?? "/");
  const adminDest = resolveAdminNext(location.search);
  const autoFollow = params.get("auto") === "1";

  const continueBtn = document.getElementById("continue-globe") as HTMLButtonElement | null;
  if (continueBtn) {
    continueBtn.hidden = !shouldShowContinue(location.search);
  }

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (!meRes.ok) {
    replace(`/login?next=${encodeURIComponent(nextAfterLogin)}`);
    return;
  }

  let initialData = (await meRes.json()) as AccountMeResponse;
  const autoActivateId = findSoleAutoActivateMembershipId(
    initialData.activeMembershipId,
    initialData.memberships ?? [],
  );
  if (autoActivateId) {
    const sw = await fetch("/api/account/switch-club", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ membershipId: autoActivateId }),
    });
    if (sw.ok) {
      const refreshed = await fetch("/api/account/me", { credentials: "same-origin" });
      if (refreshed.ok) {
        initialData = (await refreshed.json()) as AccountMeResponse;
      } else {
        initialData = { ...initialData, activeMembershipId: autoActivateId };
      }
    }
  }

  const pickPanel = document.getElementById("pick-panel") as HTMLElement;
  const joinPanel = document.getElementById("join-panel") as HTMLElement;
  const clubsMsg = document.getElementById("clubs-msg") as HTMLParagraphElement | null;

  const show = (panel: "pick" | "join") => {
    pickPanel.hidden = panel !== "pick";
    joinPanel.hidden = panel !== "join";
  };
  show("pick");

  document.getElementById("join-open")?.addEventListener("click", () => show("join"));
  document.getElementById("join-back")?.addEventListener("click", () => show("pick"));

  const logoutBtn = document.getElementById("logout-btn") as HTMLButtonElement | null;
  logoutBtn?.addEventListener("click", () => {
    logoutBtn.disabled = true;
    void performAccountLogout(replace);
  });

  const joinForm = joinFormEl;
  joinForm?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(joinForm));
    const res = await fetch("/api/account/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    const msgJ = document.getElementById("join-msg") as HTMLParagraphElement | null;
    if (res.ok) {
      if (msgJ) msgJ.textContent = t("account.join.pending");
      show("pick");
      await renderPick();
    } else {
      const err = ((await res.json().catch(() => ({}))) as { error?: string }).error ?? "join failed";
      if (msgJ) msgJ.textContent = apiErrorMessage(String(err));
    }
  });

  continueBtn?.addEventListener("click", () => {
    void tryContinueGlobe(nextAfterLogin);
  });

  async function tryContinueGlobe(dest: string): Promise<void> {
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) return;
    const data = (await me.json()) as AccountMeResponse;
    const activeId = data.activeMembershipId;
    const memberships = data.memberships ?? [];
    const active = activeId ? memberships.find((m) => m.id === activeId) : undefined;
    if (active?.status === "active") {
      navigate(dest);
      return;
    }
    const firstActive = memberships.find((m) => m.status === "active");
    if (!firstActive) return;
    const res = await fetch("/api/account/switch-club", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ membershipId: firstActive.id }),
    });
    if (res.ok) navigate(dest);
  }

  let lastData: AccountMeResponse = initialData;
  window.addEventListener("fv-locale-change", () => paintAccountMe(lastData));
  paintAccountMe(initialData);

  async function renderPick(): Promise<void> {
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) {
      replace(`/login?next=${encodeURIComponent(nextAfterLogin)}`);
      return;
    }
    const data = (await me.json()) as AccountMeResponse;
    lastData = data;
    paintAccountMe(data);
  }

  function paintAccountMe(data: AccountMeResponse): void {
    const emailEl = document.getElementById("account-email") as HTMLParagraphElement | null;
    const email = typeof data.account?.email === "string" ? data.account.email.trim() : "";
    if (emailEl) {
      emailEl.textContent = email;
      emailEl.hidden = !email;
    }

    const list = document.getElementById("club-list") as HTMLUListElement | null;
    if (!list) return;
    list.replaceChildren();

    const activeId = data.activeMembershipId ?? null;
    const memberships = data.memberships ?? [];
    if (!memberships.length) {
      const li = document.createElement("li");
      li.className = "muted";
      li.textContent = t("club.login.noClubs");
      list.appendChild(li);
      return;
    }

    for (const m of memberships) {
      const isCurrent = Boolean(activeId && m.id === activeId);
      const li = document.createElement("li");
      li.className = isCurrent ? "club-card is-current" : "club-card";
      li.dataset.membershipId = m.id;
      if (isCurrent) li.dataset.current = "true";

      const header = document.createElement("div");
      header.className = "club-card-header";

      const nameEl = document.createElement("strong");
      nameEl.className = "club-card-name";
      nameEl.textContent = m.name ?? m.slug ?? t("club.login.unnamedClub");
      header.appendChild(nameEl);

      if (isCurrent) {
        const badge = document.createElement("span");
        badge.className = "club-current-badge";
        badge.textContent = t("club.login.currentClub");
        header.appendChild(badge);
      }

      const meta = document.createElement("div");
      meta.className = "club-card-meta";
      meta.textContent = `${membershipRoleLabel(m.role)} · ${membershipStatusLabel(m.status)}`;

      li.append(header, meta);

      if (m.status === "active") {
        const actions = document.createElement("div");
        actions.className = "club-card-actions";

        if (isStaffRole(m.role)) {
          const manageBtn = document.createElement("button");
          manageBtn.type = "button";
          manageBtn.className = "btn-primary club-card-admin";
          manageBtn.textContent = t("club.login.manageClub");
          manageBtn.addEventListener("click", async () => {
            if (clubsMsg) clubsMsg.textContent = "";
            manageBtn.disabled = true;
            const res = await fetch("/api/account/switch-club", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              credentials: "same-origin",
              body: JSON.stringify({ membershipId: m.id }),
            });
            if (res.ok) {
              navigate(adminDest);
            } else {
              manageBtn.disabled = false;
              if (clubsMsg) clubsMsg.textContent = t("club.login.failed");
            }
          });
          actions.appendChild(manageBtn);

          if (!isCurrent || autoFollow) {
            const selectBtn = document.createElement("button");
            selectBtn.type = "button";
            selectBtn.className = "btn-secondary club-card-select";
            selectBtn.textContent = t("club.login.useClub");
            selectBtn.addEventListener("click", async () => {
              if (clubsMsg) clubsMsg.textContent = "";
              selectBtn.disabled = true;
              const res = await fetch("/api/account/switch-club", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                credentials: "same-origin",
                body: JSON.stringify({ membershipId: m.id }),
              });
              if (res.ok) {
                await renderPick();
                if (autoFollow) navigate(nextAfterLogin);
              } else {
                selectBtn.disabled = false;
                if (clubsMsg) clubsMsg.textContent = t("club.login.failed");
              }
            });
            actions.appendChild(selectBtn);
          }
        } else {
          const selectBtn = document.createElement("button");
          selectBtn.type = "button";
          selectBtn.className = "btn-secondary club-card-select";
          selectBtn.textContent = t("club.login.useClub");
          selectBtn.addEventListener("click", async () => {
            if (clubsMsg) clubsMsg.textContent = "";
            selectBtn.disabled = true;
            const res = await fetch("/api/account/switch-club", {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              credentials: "same-origin",
              body: JSON.stringify({ membershipId: m.id }),
            });
            if (res.ok) {
              await renderPick();
              if (autoFollow) navigate(nextAfterLogin);
            } else {
              selectBtn.disabled = false;
              if (clubsMsg) clubsMsg.textContent = t("club.login.failed");
            }
          });
          actions.appendChild(selectBtn);
        }

        li.appendChild(actions);
      }

      list.appendChild(li);
    }
  }
}
