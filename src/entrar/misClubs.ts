import { startClubPage } from "../club/i18nPage";
import { applyI18nDocument, apiErrorMessage, t } from "../i18n/index";
import { safeNextPath } from "../club/safeNext";

function staffRoles(role: string): boolean {
  return role === "admin" || role === "staff";
}

export async function bootMisClubs(): Promise<void> {
  const joinFormEl = document.getElementById("join-form") as HTMLFormElement | null;
  joinFormEl?.addEventListener("submit", (e) => e.preventDefault(), { capture: true });

  await startClubPage({ persistUser: true });
  applyI18nDocument(document);

  const params = new URLSearchParams(location.search);
  const nextAfterLogin = safeNextPath(params.get("next") ?? "/");

  const meRes = await fetch("/api/account/me", { credentials: "same-origin" });
  if (!meRes.ok) {
    location.replace(`/login?next=${encodeURIComponent(nextAfterLogin)}`);
    return;
  }

  const pickPanel = document.getElementById("pick-panel") as HTMLElement;
  const joinPanel = document.getElementById("join-panel") as HTMLElement;
  const adminPanel = document.getElementById("admin-panel") as HTMLElement;

  const show = (panel: "pick" | "join" | "admin") => {
    pickPanel.hidden = panel !== "pick";
    joinPanel.hidden = panel !== "join";
    adminPanel.hidden = panel !== "admin";
  };
  show("pick");

  document.getElementById("join-open")?.addEventListener("click", () => show("join"));
  document.getElementById("join-back")?.addEventListener("click", () => show("pick"));
  document.getElementById("admin-open")?.addEventListener("click", () => show("admin"));
  document.getElementById("admin-back")?.addEventListener("click", () => show("pick"));

  const joinForm = joinFormEl!;
  joinForm.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = Object.fromEntries(new FormData(joinForm));
    const res = await fetch("/api/account/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify(body),
    });
    const msgJ = document.getElementById("join-msg") as HTMLParagraphElement;
    if (res.ok) {
      msgJ.textContent = t("account.join.pending");
      show("pick");
      await renderPick();
    } else {
      const err = (await res.json().catch(() => ({}))).error ?? "join failed";
      msgJ.textContent = apiErrorMessage(String(err));
    }
  });

  const adminForm = document.getElementById("admin-form") as HTMLFormElement;
  adminForm?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const slug = String(new FormData(adminForm).get("slug") ?? "")
      .trim()
      .toLowerCase();
    const msgA = document.getElementById("admin-msg") as HTMLParagraphElement;
    if (!slug) return;
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) return;
    const data = await me.json();
    const memberships = (data.memberships ?? []) as Array<{
      id: string;
      slug?: string;
      status: string;
      role?: string;
    }>;
    const match = memberships.find(
      (m) =>
        (m.slug ?? "").toLowerCase() === slug &&
        m.status === "active" &&
        staffRoles(m.role ?? ""),
    );
    if (!match) {
      msgA.textContent = t("club.login.staffDenied");
      return;
    }
    const res = await fetch("/api/account/switch-club", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: JSON.stringify({ membershipId: match.id }),
    });
    if (res.ok) {
      location.href = safeNextPath(
        params.get("adminNext") ?? "/club-admin-map.html",
      );
    } else {
      msgA.textContent = t("club.login.failed");
    }
  });

  document.getElementById("continue-globe")?.addEventListener("click", () => {
    void tryContinueGlobe(nextAfterLogin);
  });

  async function tryContinueGlobe(dest: string): Promise<void> {
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) return;
    const data = await me.json();
    const activeId = data.activeMembershipId as string | null | undefined;
    const memberships = (data.memberships ?? []) as Array<{ id: string; status: string }>;
    const active = activeId ? memberships.find((m) => m.id === activeId) : undefined;
    if (active?.status === "active") {
      location.href = dest;
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
    if (res.ok) location.href = dest;
  }

  await renderPick();

  async function renderPick() {
    const list = document.getElementById("club-list") as HTMLUListElement;
    list.innerHTML = "";
    const me = await fetch("/api/account/me", { credentials: "same-origin" });
    if (!me.ok) {
      location.replace(`/login?next=${encodeURIComponent(nextAfterLogin)}`);
      return;
    }
    const data = await me.json();
    const activeId = data.activeMembershipId as string | null | undefined;
    const memberships = (data.memberships ?? []) as Array<{
      id: string;
      name?: string;
      slug?: string;
      status: string;
      role?: string;
    }>;
    if (!memberships.length) {
      const li = document.createElement("li");
      li.className = "muted";
      li.textContent = t("club.login.noClubs");
      list.appendChild(li);
      return;
    }
    for (const m of memberships) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      const label = m.name ?? m.slug ?? m.id;
      const activeMark = m.id === activeId ? " · ✓" : "";
      btn.textContent = `${label} (${m.status})${activeMark}`;
      btn.disabled = m.status !== "active";
      btn.addEventListener("click", async () => {
        const res = await fetch("/api/account/switch-club", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          credentials: "same-origin",
          body: JSON.stringify({ membershipId: m.id }),
        });
        if (res.ok) {
          await renderPick();
          if (params.get("auto") === "1") location.href = nextAfterLogin;
        }
      });
      li.appendChild(btn);
      list.appendChild(li);
    }
  }
}